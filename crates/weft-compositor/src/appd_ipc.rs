#[cfg(unix)]
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
};

#[cfg(unix)]
use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};

#[cfg(unix)]
use weft_ipc_types::{
    AppdToCompositor, CompositorToAppd, MAX_FRAME_LEN, frame_decode, frame_encode,
};

#[cfg(unix)]
use crate::state::WeftCompositorState;

#[cfg(unix)]
pub struct WeftAppdIpc {
    pub socket_path: PathBuf,
    read_buf: Vec<u8>,
    write_stream: Option<UnixStream>,
    /// pid → (app_id, session_id) for Wayland clients whose surface has not yet arrived.
    pub pending_pids: HashMap<u32, (String, u64)>,
}

#[cfg(unix)]
impl WeftAppdIpc {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            read_buf: Vec::new(),
            write_stream: None,
            pending_pids: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn send(&mut self, msg: &CompositorToAppd) {
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

    pub fn on_read(&mut self, stream: &mut UnixStream) -> (Vec<AppdToCompositor>, bool) {
        let mut buf = [0u8; 8192];
        let mut eof = false;
        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => self.read_buf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    tracing::warn!(?e, "appd IPC stream read error");
                    eof = true;
                    break;
                }
            }
        }
        let messages = self.try_decode_frames();
        if eof {
            self.write_stream = None;
            self.read_buf.clear();
        }
        (messages, eof)
    }
}

#[cfg(unix)]
fn handle_message(state: &mut WeftCompositorState, msg: AppdToCompositor) {
    match msg {
        AppdToCompositor::AppSurfaceCreated {
            app_id,
            session_id,
            pid,
        } => {
            tracing::debug!(app_id, session_id, pid, "AppSurfaceCreated");
            if let Some(ipc) = &mut state.appd_ipc {
                ipc.pending_pids.insert(pid, (app_id, session_id));
            }
        }
        AppdToCompositor::AppSurfaceDestroyed { session_id } => {
            tracing::debug!(session_id, "AppSurfaceDestroyed");
        }
        AppdToCompositor::AppFocusRequest { session_id } => {
            tracing::debug!(session_id, "AppFocusRequest");
        }
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

    let handle = state.loop_handle.clone();
    state
        .loop_handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _addr)) => {
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
