#[cfg(unix)]
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    os::fd::OwnedFd,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::Arc,
};

#[cfg(unix)]
use smithay::reexports::calloop::{Interest, Mode, PostAction, channel, generic::Generic};
#[cfg(unix)]
use smithay::reexports::wayland_server::backend::{ClientId, DisconnectReason};

#[cfg(unix)]
use weft_ipc_types::{
    AppdToCompositor, CompositorToAppd, MAX_FRAME_LEN, frame_decode, frame_encode,
};

#[cfg(unix)]
use crate::state::{SessionBinding, WeftClientState, WeftCompositorState};

/// Descriptors received ahead of the messages that claim them; more than
/// this means the peer is not following the protocol.
#[cfg(unix)]
const MAX_PENDING_FDS: usize = 16;

#[cfg(unix)]
pub struct WeftAppdIpc {
    pub socket_path: PathBuf,
    read_buf: Vec<u8>,
    write_stream: Option<UnixStream>,
    /// Descriptors received with appd messages, in arrival order. Each
    /// `AttachClient` frame is sent with exactly one, attached to its first
    /// bytes, so it has arrived by the time the frame is decoded.
    fds: VecDeque<OwnedFd>,
    /// The Wayland client of each appd session.
    sessions: HashMap<u64, ClientId>,
    /// Sessions whose first toplevel was reported to appd.
    reported: HashSet<u64>,
    /// Where clients report their session's disconnection.
    disconnected: Option<channel::Sender<u64>>,
}

#[cfg(unix)]
impl WeftAppdIpc {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            read_buf: Vec::new(),
            write_stream: None,
            fds: VecDeque::new(),
            sessions: HashMap::new(),
            reported: HashSet::new(),
            disconnected: None,
        }
    }

    pub fn send(&mut self, msg: &CompositorToAppd) {
        use std::io::Write;
        let Some(stream) = &mut self.write_stream else {
            return;
        };
        match frame_encode(msg) {
            Ok(frame) => {
                if stream.write_all(&frame).is_err() {
                    self.write_stream = None;
                }
            }
            Err(e) => tracing::warn!(?e, "failed to encode compositor IPC message"),
        }
    }

    /// Reports a session's first toplevel to appd.
    pub fn surface_created(&mut self, session_id: u64) {
        if self.sessions.contains_key(&session_id) && self.reported.insert(session_id) {
            self.send(&CompositorToAppd::SurfaceReady { session_id });
        }
    }

    fn try_decode_frames(&mut self) -> Vec<AppdToCompositor> {
        let mut out = Vec::new();
        loop {
            if self.read_buf.len() < 4 {
                break;
            }
            let declared_len = u32::from_le_bytes([
                self.read_buf[0],
                self.read_buf[1],
                self.read_buf[2],
                self.read_buf[3],
            ]) as usize;
            if declared_len > MAX_FRAME_LEN {
                tracing::warn!(declared_len, "appd IPC frame too large; disconnecting");
                self.write_stream = None;
                self.read_buf.clear();
                break;
            }
            if self.read_buf.len() < 4 + declared_len {
                break;
            }
            let frame_end = 4 + declared_len;
            match frame_decode::<AppdToCompositor>(&self.read_buf[..frame_end]) {
                Ok(msg) => out.push(msg),
                Err(e) => tracing::warn!(?e, "appd IPC frame decode error"),
            }
            self.read_buf.drain(..frame_end);
        }
        out
    }

    /// Reads what is available, keeping descriptors that arrive with the
    /// data, and returns the complete messages and whether appd hung up.
    pub fn on_read(&mut self, stream: &UnixStream) -> (Vec<AppdToCompositor>, bool) {
        use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
        use std::mem::MaybeUninit;
        let mut buf = [0u8; 8192];
        let mut eof = false;
        loop {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(4))];
            let mut control = RecvAncillaryBuffer::new(&mut space);
            let received = recvmsg(
                stream,
                &mut [io::IoSliceMut::new(&mut buf)],
                &mut control,
                RecvFlags::CMSG_CLOEXEC | RecvFlags::DONTWAIT,
            );
            for message in control.drain() {
                if let RecvAncillaryMessage::ScmRights(fds) = message {
                    self.fds.extend(fds);
                }
            }
            match received {
                Ok(msg) if msg.bytes == 0 => {
                    eof = true;
                    break;
                }
                Ok(msg) => self.read_buf.extend_from_slice(&buf[..msg.bytes]),
                Err(rustix::io::Errno::AGAIN) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => {
                    tracing::warn!(?e, "appd IPC stream read error");
                    eof = true;
                    break;
                }
            }
        }
        if self.fds.len() > MAX_PENDING_FDS {
            tracing::warn!("appd sent descriptors without messages; disconnecting");
            eof = true;
        }
        let messages = self.try_decode_frames();
        if eof {
            self.write_stream = None;
            self.read_buf.clear();
            self.fds.clear();
        }
        (messages, eof)
    }
}

#[cfg(unix)]
fn handle_message(state: &mut WeftCompositorState, msg: AppdToCompositor) {
    match msg {
        AppdToCompositor::AttachClient { session_id, app_id } => {
            attach_client(state, session_id, app_id);
        }
        AppdToCompositor::AppSurfaceDestroyed { session_id } => {
            let Some(ipc) = state.appd_ipc.as_mut() else {
                return;
            };
            ipc.reported.remove(&session_id);
            if let Some(client) = ipc.sessions.remove(&session_id) {
                tracing::info!(session_id, "closing the client of an ended session");
                state
                    .display_handle
                    .backend_handle()
                    .kill_client(client, DisconnectReason::ConnectionClosed);
            }
        }
        AppdToCompositor::AppFocusRequest { session_id } => {
            tracing::debug!(session_id, "AppFocusRequest");
        }
    }
}

/// Makes the descriptor that came with an `AttachClient` message a Wayland
/// client bound to `session_id`.
#[cfg(unix)]
fn attach_client(state: &mut WeftCompositorState, session_id: u64, app_id: String) {
    let Some(ipc) = state.appd_ipc.as_mut() else {
        return;
    };
    let Some(fd) = ipc.fds.pop_front() else {
        tracing::warn!(
            session_id,
            "AttachClient arrived without a connection; ignored"
        );
        return;
    };
    if !weft_ipc_types::package::is_valid_app_id(&app_id) {
        tracing::warn!(session_id, "AttachClient with an invalid app ID; ignored");
        return;
    }
    if ipc.sessions.contains_key(&session_id) {
        tracing::warn!(
            session_id,
            "session already has a client; connection refused"
        );
        return;
    }
    let Some(disconnected) = ipc.disconnected.clone() else {
        return;
    };
    let stream = UnixStream::from(fd);
    let data = Arc::new(WeftClientState {
        session: Some(SessionBinding {
            session_id,
            app_id: app_id.clone(),
            disconnected,
        }),
        ..WeftClientState::default()
    });
    match state.display_handle.insert_client(stream, data) {
        Ok(client) => {
            tracing::info!(session_id, %app_id, "client bound to session");
            if let Some(ipc) = state.appd_ipc.as_mut() {
                ipc.sessions.insert(session_id, client.id());
            }
        }
        Err(e) => tracing::warn!(session_id, ?e, "cannot add the session's client"),
    }
}

#[cfg(unix)]
pub fn compositor_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("WEFT_COMPOSITOR_SOCKET") {
        return PathBuf::from(p);
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("weft").join("compositor.sock");
    }
    PathBuf::from("/tmp/weft-compositor.sock")
}

#[cfg(unix)]
pub fn setup(state: &mut WeftCompositorState) -> anyhow::Result<()> {
    use anyhow::Context;

    let socket_path = {
        let ipc = state
            .appd_ipc
            .as_ref()
            .context("appd_ipc not initialised")?;
        ipc.socket_path.clone()
    };

    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).context("create compositor IPC socket directory")?;
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind compositor IPC socket at {}", socket_path.display()))?;
    listener.set_nonblocking(true)?;

    tracing::info!(path = %socket_path.display(), "compositor IPC socket open");

    // Clients bound to a session report their disconnection here.
    let (sender, receiver) = channel::channel::<u64>();
    if let Some(ipc) = state.appd_ipc.as_mut() {
        ipc.disconnected = Some(sender);
    }
    state
        .loop_handle
        .insert_source(receiver, |event, _, state| {
            if let channel::Event::Msg(session_id) = event
                && let Some(ipc) = state.appd_ipc.as_mut()
            {
                ipc.sessions.remove(&session_id);
                ipc.reported.remove(&session_id);
                tracing::info!(session_id, "session client disconnected");
                ipc.send(&CompositorToAppd::ClientDisconnected { session_id });
            }
        })
        .map_err(|e| anyhow::anyhow!("insert session disconnect source: {e}"))?;

    let handle = state.loop_handle.clone();
    state
        .loop_handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _addr)) => {
                            // One appd at a time: a second connection while
                            // one is live is refused rather than replacing it.
                            if state
                                .appd_ipc
                                .as_ref()
                                .is_some_and(|ipc| ipc.write_stream.is_some())
                            {
                                tracing::warn!(
                                    "second compositor IPC connection refused; weft-appd is connected"
                                );
                                continue;
                            }
                            tracing::info!("weft-appd connected to compositor IPC");
                            let write_clone = match stream.try_clone() {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!(?e, "try_clone failed for appd IPC stream");
                                    continue;
                                }
                            };
                            if let Some(ipc) = &mut state.appd_ipc {
                                ipc.write_stream = Some(write_clone);
                                ipc.read_buf.clear();
                                ipc.fds.clear();
                            }
                            stream.set_nonblocking(true).ok();
                            let _ = handle.insert_source(
                                Generic::new(stream, Interest::READ, Mode::Edge),
                                |_, stream, state| {
                                    // Safety: calloop wraps the fd in NoIoDrop to prevent
                                    // accidental drops; get_mut gives the inner stream.
                                    let inner: &mut UnixStream = unsafe { stream.get_mut() };
                                    let (messages, eof) = match state.appd_ipc.as_mut() {
                                        Some(ipc) => ipc.on_read(inner),
                                        None => return Ok(PostAction::Remove),
                                    };
                                    for msg in messages {
                                        handle_message(state, msg);
                                    }
                                    if eof {
                                        tracing::info!(
                                            "weft-appd disconnected from compositor IPC"
                                        );
                                        Ok(PostAction::Remove)
                                    } else {
                                        Ok(PostAction::Continue)
                                    }
                                },
                            );
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => tracing::warn!(?e, "accept error on compositor IPC socket"),
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("insert compositor IPC listener source: {e}"))?;

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
    use std::mem::MaybeUninit;
    use std::os::fd::AsFd;

    fn send_with_fd(stream: &UnixStream, bytes: &[u8], fd: &impl AsFd) {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        let fds = [fd.as_fd()];
        assert!(control.push(SendAncillaryMessage::ScmRights(&fds)));
        let sent = sendmsg(
            stream,
            &[io::IoSlice::new(bytes)],
            &mut control,
            SendFlags::empty(),
        )
        .unwrap();
        assert_eq!(sent, bytes.len());
    }

    #[test]
    fn a_connection_arrives_with_the_message_that_claims_it() {
        let (appd, compositor) = UnixStream::pair().unwrap();
        compositor.set_nonblocking(true).unwrap();
        let (shell, client) = UnixStream::pair().unwrap();
        let frame = frame_encode(&AppdToCompositor::AttachClient {
            session_id: 7,
            app_id: "org.weft.demo.counter".into(),
        })
        .unwrap();
        send_with_fd(&appd, &frame, &client);
        drop(client);

        let mut ipc = WeftAppdIpc::new(PathBuf::from("/nonexistent"));
        let (messages, eof) = ipc.on_read(&compositor);
        assert!(!eof);
        assert!(matches!(
            messages.as_slice(),
            [AppdToCompositor::AttachClient { session_id: 7, .. }]
        ));
        // The received descriptor is the other end of the shell's connection.
        let received = UnixStream::from(ipc.fds.pop_front().expect("no descriptor"));
        use std::io::{Read, Write};
        (&shell).write_all(b"hello").unwrap();
        let mut buf = [0u8; 5];
        (&received).read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn descriptors_without_messages_end_the_connection() {
        let (appd, compositor) = UnixStream::pair().unwrap();
        compositor.set_nonblocking(true).unwrap();
        let (_shell, client) = UnixStream::pair().unwrap();
        let mut ipc = WeftAppdIpc::new(PathBuf::from("/nonexistent"));
        for _ in 0..=MAX_PENDING_FDS {
            send_with_fd(&appd, &[0], &client);
        }
        let (messages, eof) = ipc.on_read(&compositor);
        assert!(messages.is_empty());
        assert!(eof);
        assert!(ipc.fds.is_empty());
    }
}
