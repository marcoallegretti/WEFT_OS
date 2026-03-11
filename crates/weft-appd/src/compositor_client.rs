use std::path::PathBuf;

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use weft_ipc_types::{
    AppdToCompositor, CompositorToAppd, MAX_FRAME_LEN, frame_decode, frame_encode,
};

pub type CompositorSender = mpsc::Sender<AppdToCompositor>;

/// Resolve the compositor IPC socket path.
///
/// Returns `None` if neither `WEFT_COMPOSITOR_SOCKET` nor `XDG_RUNTIME_DIR` is set,
/// meaning no compositor IPC is available in this environment.
pub fn socket_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WEFT_COMPOSITOR_SOCKET") {
        return Some(PathBuf::from(p));
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return Some(PathBuf::from(dir).join("weft").join("compositor.sock"));
    }
    None
}

/// Spawn the compositor IPC client task and return a sender for outbound messages.
///
/// The task connects to `socket_path`, retrying every 2 s on failure. If the
/// connection drops while sending, it waits 500 ms then reconnects. Incoming
/// `CompositorToAppd` frames are decoded and logged; no behavioural action is
/// taken yet (surface lifecycle hookup happens in a later task).
pub fn spawn(socket_path: PathBuf) -> CompositorSender {
    let (tx, rx) = mpsc::channel::<AppdToCompositor>(32);
    tokio::spawn(run_client(socket_path, rx));
    tx
}

async fn run_client(socket_path: PathBuf, mut rx: mpsc::Receiver<AppdToCompositor>) {
    loop {
        let stream = loop {
            match tokio::net::UnixStream::connect(&socket_path).await {
                Ok(s) => {
                    tracing::info!(path = %socket_path.display(), "connected to compositor IPC");
                    break s;
                }
                Err(_) => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            }
        };

        let (read_half, mut write_half) = stream.into_split();

        tokio::spawn(read_unix_incoming(read_half));

        loop {
            let Some(msg) = rx.recv().await else {
                return;
            };
            match frame_encode(&msg) {
                Ok(frame) => {
                    if write_half.write_all(&frame).await.is_err() {
                        tracing::warn!("compositor IPC write failed; reconnecting");
                        break;
                    }
                }
                Err(e) => tracing::warn!(?e, "compositor IPC encode error"),
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }
}

async fn read_unix_incoming(mut reader: tokio::net::unix::OwnedReadHalf) {
    use tokio::io::AsyncReadExt;

    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match reader.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                loop {
                    if buf.len() < 4 {
                        break;
                    }
                    let declared_len =
                        u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                    if declared_len > MAX_FRAME_LEN {
                        buf.clear();
                        break;
                    }
                    if buf.len() < 4 + declared_len {
                        break;
                    }
                    let frame_end = 4 + declared_len;
                    if let Ok(msg) = frame_decode::<CompositorToAppd>(&buf[..frame_end]) {
                        handle_incoming(msg);
                    }
                    buf.drain(..frame_end);
                }
            }
        }
    }
}

fn handle_incoming(msg: CompositorToAppd) {
    match msg {
        CompositorToAppd::SurfaceReady { session_id } => {
            tracing::debug!(session_id, "SurfaceReady from compositor");
        }
        CompositorToAppd::ClientDisconnected { pid } => {
            tracing::debug!(pid, "ClientDisconnected from compositor");
        }
    }
}
