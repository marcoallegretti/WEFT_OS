use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use weft_ipc_types::{
    AppdToCompositor, CompositorToAppd, MAX_FRAME_LEN, frame_decode, frame_encode,
};

/// A message for the compositor and the file descriptor it carries, if any.
pub struct Outbound {
    pub msg: AppdToCompositor,
    pub fd: Option<OwnedFd>,
}

impl From<AppdToCompositor> for Outbound {
    fn from(msg: AppdToCompositor) -> Self {
        Self { msg, fd: None }
    }
}

/// Queues messages for the compositor and tells whether it is connected.
#[derive(Clone)]
pub struct CompositorSender {
    tx: mpsc::Sender<Outbound>,
    connected: Arc<AtomicBool>,
}

impl CompositorSender {
    pub fn new(tx: mpsc::Sender<Outbound>, connected: Arc<AtomicBool>) -> Self {
        Self { tx, connected }
    }

    /// Whether the compositor IPC connection is currently up.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn try_send(&self, outbound: Outbound) -> Result<(), Outbound> {
        self.tx.try_send(outbound).map_err(|e| e.into_inner())
    }
}

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
/// The task connects to `socket_path`, retrying every 2 s on failure. When
/// the connection drops, whether noticed by a failed write or by the
/// compositor closing it, it waits 500 ms and reconnects; a message whose
/// write failed is sent again on the new connection. Incoming
/// `CompositorToAppd` frames are decoded and logged.
pub fn spawn(socket_path: PathBuf) -> CompositorSender {
    let (tx, rx) = mpsc::channel::<Outbound>(32);
    let connected = Arc::new(AtomicBool::new(false));
    tokio::spawn(run_client(socket_path, rx, connected.clone()));
    CompositorSender::new(tx, connected)
}

async fn run_client(
    socket_path: PathBuf,
    mut rx: mpsc::Receiver<Outbound>,
    connected: Arc<AtomicBool>,
) {
    let mut unsent: Option<Outbound> = None;
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
        let mut reader = tokio::spawn(read_unix_incoming(read_half));
        connected.store(true, Ordering::Release);

        loop {
            let outbound = match unsent.take() {
                Some(outbound) => outbound,
                None => tokio::select! {
                    next = rx.recv() => match next {
                        Some(outbound) => outbound,
                        None => return,
                    },
                    _ = &mut reader => {
                        tracing::warn!("compositor closed the IPC connection; reconnecting");
                        break;
                    }
                },
            };
            let frame = match frame_encode(&outbound.msg) {
                Ok(frame) => frame,
                Err(e) => {
                    tracing::warn!(?e, "compositor IPC encode error");
                    continue;
                }
            };
            if send_frame(&mut write_half, &frame, outbound.fd.as_ref())
                .await
                .is_err()
            {
                tracing::warn!("compositor IPC write failed; reconnecting");
                unsent = Some(outbound);
                break;
            }
        }
        connected.store(false, Ordering::Release);
        reader.abort();

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }
}

/// Writes `frame`, attaching `fd` to its first bytes so the compositor
/// receives the descriptor together with the message it belongs to.
async fn send_frame(
    stream: &mut tokio::net::unix::OwnedWriteHalf,
    frame: &[u8],
    fd: Option<&OwnedFd>,
) -> std::io::Result<()> {
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
    use std::mem::MaybeUninit;
    use std::os::fd::AsFd;
    let Some(fd) = fd else {
        return stream.write_all(frame).await;
    };
    let socket: &tokio::net::UnixStream = stream.as_ref();
    let sent = loop {
        socket.writable().await?;
        let result = socket.try_io(tokio::io::Interest::WRITABLE, || {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut control = SendAncillaryBuffer::new(&mut space);
            let fds = [fd.as_fd()];
            control.push(SendAncillaryMessage::ScmRights(&fds));
            sendmsg(
                socket,
                &[std::io::IoSlice::new(frame)],
                &mut control,
                SendFlags::NOSIGNAL,
            )
            .map_err(std::io::Error::from)
        });
        match result {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            other => break other?,
        }
    };
    // The descriptor travelled with the first bytes; the rest of a partly
    // sent frame follows as plain data.
    stream.write_all(&frame[sent..]).await
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
        CompositorToAppd::ClientDisconnected { session_id } => {
            tracing::debug!(session_id, "ClientDisconnected from compositor");
        }
    }
}
