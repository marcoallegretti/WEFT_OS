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
use smithay::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction, RegistrationToken, channel,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};
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
    /// Whether an appd connection's read source is registered. Only one
    /// connection is served at a time.
    connected: bool,
    /// Descriptors received with appd messages, in arrival order. Each
    /// `AttachClient` frame is sent with exactly one, attached to its first
    /// bytes, so it has arrived by the time the frame is decoded.
    fds: VecDeque<OwnedFd>,
    /// The Wayland client of each appd session.
    sessions: HashMap<u64, ClientId>,
    /// Sessions whose first toplevel was reported to appd.
    reported: HashSet<u64>,
    /// Sessions that already received keyboard focus for a mapped window;
    /// kept until the session ends, so recreating a window gains nothing.
    focused_on_map: HashSet<u64>,
    /// Where clients report their session's disconnection.
    disconnected: Option<channel::Sender<(u64, ClientId)>>,
    /// Bytes for appd that did not fit in the socket buffer, sent before
    /// anything else so frames stay whole.
    out_buf: Vec<u8>,
    /// Retries `flush` while `out_buf` holds bytes; armed only then.
    flush_retry: Option<RegistrationToken>,
    loop_handle: Option<LoopHandle<'static, WeftCompositorState>>,
}

#[cfg(unix)]
impl WeftAppdIpc {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            read_buf: Vec::new(),
            write_stream: None,
            connected: false,
            fds: VecDeque::new(),
            sessions: HashMap::new(),
            reported: HashSet::new(),
            focused_on_map: HashSet::new(),
            disconnected: None,
            out_buf: Vec::new(),
            flush_retry: None,
            loop_handle: None,
        }
    }

    pub fn send(&mut self, msg: &CompositorToAppd) {
        match frame_encode(msg) {
            Ok(frame) => {
                if self.write_stream.is_some() {
                    self.out_buf.extend_from_slice(&frame);
                    self.flush();
                    self.schedule_flush();
                }
            }
            Err(e) => tracing::warn!(?e, "failed to encode compositor IPC message"),
        }
    }

    /// Writes what appd's socket accepts now; the rest waits for the next
    /// send or read. A backlog appd never drains, or a write error, ends the
    /// connection, whose read side then cleans up.
    fn flush(&mut self) {
        use std::io::Write;
        let Some(stream) = &mut self.write_stream else {
            self.out_buf.clear();
            return;
        };
        while !self.out_buf.is_empty() {
            match stream.write(&self.out_buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.out_buf.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        "compositor IPC write failed; closing the appd connection"
                    );
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    self.write_stream = None;
                    self.out_buf.clear();
                    return;
                }
            }
        }
        if self.out_buf.len() > MAX_FRAME_LEN {
            tracing::warn!("weft-appd is not reading compositor IPC; closing the connection");
            let _ = stream.shutdown(std::net::Shutdown::Both);
            self.write_stream = None;
            self.out_buf.clear();
        }
    }

    /// Retries the flush until appd has taken every queued byte, so the
    /// last frames of a burst do not wait for unrelated traffic.
    fn schedule_flush(&mut self) {
        if self.out_buf.is_empty() || self.flush_retry.is_some() {
            return;
        }
        let Some(handle) = &self.loop_handle else {
            return;
        };
        let retry = Timer::from_duration(std::time::Duration::from_millis(20));
        self.flush_retry = handle
            .insert_source(retry, |_, _, state| {
                let Some(ipc) = state.appd_ipc.as_mut() else {
                    return TimeoutAction::Drop;
                };
                ipc.flush();
                if ipc.out_buf.is_empty() {
                    ipc.flush_retry = None;
                    TimeoutAction::Drop
                } else {
                    TimeoutAction::ToDuration(std::time::Duration::from_millis(20))
                }
            })
            .ok();
    }

    /// Whether a window `session_id` maps now may take keyboard focus: only
    /// the first one in the session's lifetime.
    pub fn take_focus_on_map(&mut self, session_id: u64) -> bool {
        self.sessions.contains_key(&session_id) && self.focused_on_map.insert(session_id)
    }

    /// Reports a session's first toplevel to appd.
    pub fn surface_created(&mut self, session_id: u64) {
        if self.sessions.contains_key(&session_id) && self.reported.insert(session_id) {
            self.send(&CompositorToAppd::SurfaceReady { session_id });
        }
    }

    /// Decodes the complete frames in the buffer, and reports whether the
    /// stream can no longer be trusted: after an oversized or undecodable
    /// frame, descriptors could be paired with the wrong messages.
    fn try_decode_frames(&mut self) -> (Vec<AppdToCompositor>, bool) {
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
                return (out, true);
            }
            if self.read_buf.len() < 4 + declared_len {
                break;
            }
            let frame_end = 4 + declared_len;
            match frame_decode::<AppdToCompositor>(&self.read_buf[..frame_end]) {
                Ok(msg) => out.push(msg),
                Err(e) => {
                    tracing::warn!(?e, "appd IPC frame decode error; disconnecting");
                    return (out, true);
                }
            }
            self.read_buf.drain(..frame_end);
        }
        (out, false)
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
        let (messages, broken) = self.try_decode_frames();
        eof |= broken;
        let claimed = messages
            .iter()
            .filter(|m| matches!(m, AppdToCompositor::AttachClient { .. }))
            .count();
        if self.fds.len().saturating_sub(claimed) > MAX_PENDING_FDS {
            tracing::warn!("appd sent descriptors without messages; disconnecting");
            eof = true;
        }
        (messages, eof)
    }

    /// Forgets the appd connection once its read source is gone. The
    /// sessions it started are closed: they belong to that appd.
    fn disconnect(&mut self) -> Vec<ClientId> {
        self.connected = false;
        self.write_stream = None;
        self.out_buf.clear();
        self.read_buf.clear();
        self.fds.clear();
        self.reported.clear();
        self.focused_on_map.clear();
        self.sessions.drain().map(|(_, client)| client).collect()
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
            ipc.focused_on_map.remove(&session_id);
            if let Some(client) = ipc.sessions.remove(&session_id) {
                tracing::info!(session_id, "closing the client of an ended session");
                state
                    .display_handle
                    .backend_handle()
                    .kill_client(client, DisconnectReason::ConnectionClosed);
            }
        }
        AppdToCompositor::AppCloseRequest { session_id } => {
            let closed = state.close_session_windows(session_id);
            tracing::info!(session_id, windows = closed, "session asked to close");
        }
        AppdToCompositor::AppFocusRequest { session_id } => {
            if state.activate_session(session_id) {
                tracing::info!(session_id, "session activated");
            } else {
                tracing::info!(
                    session_id,
                    "activation requested for a session with no window"
                );
            }
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

    if let Some(ipc) = state.appd_ipc.as_mut() {
        ipc.loop_handle = Some(state.loop_handle.clone());
    }
    // Clients bound to a session report their disconnection here.
    let (sender, receiver) = channel::channel::<(u64, ClientId)>();
    if let Some(ipc) = state.appd_ipc.as_mut() {
        ipc.disconnected = Some(sender);
    }
    state
        .loop_handle
        .insert_source(receiver, |event, _, state| {
            // Only the session's current client: a client closed when its
            // appd went away may report after a new appd reused the ID.
            if let channel::Event::Msg((session_id, client)) = event
                && let Some(ipc) = state.appd_ipc.as_mut()
                && ipc.sessions.get(&session_id) == Some(&client)
            {
                ipc.sessions.remove(&session_id);
                ipc.reported.remove(&session_id);
                ipc.focused_on_map.remove(&session_id);
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
                            if state.appd_ipc.as_ref().is_some_and(|ipc| ipc.connected) {
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
                                ipc.connected = true;
                                ipc.read_buf.clear();
                                ipc.fds.clear();
                            }
                            stream.set_nonblocking(true).ok();
                            let registered = handle.insert_source(
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
                                    if let Some(ipc) = state.appd_ipc.as_mut() {
                                        ipc.flush();
                                    }
                                    if eof {
                                        tracing::info!(
                                            "weft-appd disconnected from compositor IPC; \
                                             closing its sessions' clients"
                                        );
                                        let orphans = state
                                            .appd_ipc
                                            .as_mut()
                                            .map(WeftAppdIpc::disconnect)
                                            .unwrap_or_default();
                                        for client in orphans {
                                            state.display_handle.backend_handle().kill_client(
                                                client,
                                                DisconnectReason::ConnectionClosed,
                                            );
                                        }
                                        Ok(PostAction::Remove)
                                    } else {
                                        Ok(PostAction::Continue)
                                    }
                                },
                            );
                            if let Err(e) = registered {
                                tracing::warn!(?e, "cannot watch the appd IPC connection");
                                if let Some(ipc) = state.appd_ipc.as_mut() {
                                    ipc.disconnect();
                                }
                            }
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
        ipc.disconnect();
        assert!(ipc.fds.is_empty());
    }

    #[test]
    fn a_burst_of_attachments_is_not_mistaken_for_abuse() {
        let (appd, compositor) = UnixStream::pair().unwrap();
        compositor.set_nonblocking(true).unwrap();
        let mut keep = Vec::new();
        for session_id in 0..(MAX_PENDING_FDS as u64 + 4) {
            let (shell, client) = UnixStream::pair().unwrap();
            let frame = frame_encode(&AppdToCompositor::AttachClient {
                session_id,
                app_id: "org.weft.demo.counter".into(),
            })
            .unwrap();
            send_with_fd(&appd, &frame, &client);
            // A plain frame between attachments keeps its place.
            let plain = frame_encode(&AppdToCompositor::AppFocusRequest { session_id }).unwrap();
            use std::io::Write;
            (&appd).write_all(&plain).unwrap();
            keep.push(shell);
        }
        let mut ipc = WeftAppdIpc::new(PathBuf::from("/nonexistent"));
        let mut messages = Vec::new();
        let mut connections = 0;
        loop {
            let (batch, eof) = ipc.on_read(&compositor);
            assert!(!eof, "a legitimate burst ended the connection");
            if batch.is_empty() {
                break;
            }
            // As the handler does, each attachment takes its connection.
            for message in &batch {
                if matches!(message, AppdToCompositor::AttachClient { .. }) {
                    ipc.fds
                        .pop_front()
                        .expect("an attachment without its connection");
                    connections += 1;
                }
            }
            messages.extend(batch);
        }
        assert_eq!(connections, MAX_PENDING_FDS + 4);
        assert_eq!(messages.len(), 2 * (MAX_PENDING_FDS + 4));
        assert!(ipc.fds.is_empty());
    }

    #[test]
    fn a_slow_appd_gets_whole_frames_later_instead_of_losing_its_apps() {
        let (appd, compositor) = UnixStream::pair().unwrap();
        compositor.set_nonblocking(true).unwrap();
        let mut ipc = WeftAppdIpc::new(PathBuf::from("/nonexistent"));
        ipc.write_stream = Some(compositor);
        ipc.connected = true;
        // More than the socket buffer holds while appd is not reading.
        let count = 20_000u64;
        for session_id in 0..count {
            ipc.send(&CompositorToAppd::SurfaceReady { session_id });
        }
        assert!(
            ipc.write_stream.is_some(),
            "a full buffer closed the connection"
        );
        assert!(!ipc.out_buf.is_empty());

        // appd reads; every flush moves more, and every frame arrives whole.
        appd.set_nonblocking(true).unwrap();
        let mut received = Vec::new();
        let mut buf = [0u8; 65536];
        loop {
            ipc.flush();
            use std::io::Read;
            match (&appd).read(&mut buf) {
                Ok(n) => received.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if ipc.out_buf.is_empty() {
                        break;
                    }
                }
                Err(e) => panic!("{e}"),
            }
        }
        let mut frames = 0u64;
        while !received.is_empty() {
            let len = u32::from_le_bytes(received[..4].try_into().unwrap()) as usize;
            let frame: CompositorToAppd = frame_decode(&received[..4 + len]).unwrap();
            assert!(
                matches!(frame, CompositorToAppd::SurfaceReady { session_id } if session_id == frames)
            );
            received.drain(..4 + len);
            frames += 1;
        }
        assert_eq!(frames, count);
    }

    #[test]
    fn an_undecodable_frame_ends_the_connection() {
        let (appd, compositor) = UnixStream::pair().unwrap();
        compositor.set_nonblocking(true).unwrap();
        use std::io::Write;
        let mut frame = 3u32.to_le_bytes().to_vec();
        frame.extend_from_slice(&[0xc1, 0xc1, 0xc1]);
        (&appd).write_all(&frame).unwrap();
        let mut ipc = WeftAppdIpc::new(PathBuf::from("/nonexistent"));
        let (_, eof) = ipc.on_read(&compositor);
        assert!(eof);
    }
}
