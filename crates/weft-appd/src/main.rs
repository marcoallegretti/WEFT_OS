use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

mod ipc;
mod ws;

use ipc::{AppStateKind, Request, Response};

pub(crate) type Registry = Arc<Mutex<SessionRegistry>>;

#[derive(Default)]
struct SessionRegistry {
    next_id: u64,
    sessions: std::collections::HashMap<u64, AppStateKind>,
}

impl SessionRegistry {
    fn launch(&mut self, _app_id: &str) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.sessions.insert(id, AppStateKind::Starting);
        id
    }

    fn terminate(&mut self, session_id: u64) -> bool {
        self.sessions.remove(&session_id).is_some()
    }

    fn running_ids(&self) -> Vec<u64> {
        self.sessions.keys().copied().collect()
    }

    fn state(&self, session_id: u64) -> AppStateKind {
        self.sessions
            .get(&session_id)
            .cloned()
            .unwrap_or(AppStateKind::NotFound)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    run().await
}

async fn run() -> anyhow::Result<()> {
    let socket_path = appd_socket_path()?;

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&socket_path);

    let listener = tokio::net::UnixListener::bind(&socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    tracing::info!(path = %socket_path.display(), "IPC socket listening");

    let registry: Registry = Arc::new(Mutex::new(SessionRegistry::default()));
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<Response>(16);

    let ws_port = ws_port();
    let ws_addr: std::net::SocketAddr = format!("127.0.0.1:{ws_port}").parse()?;
    let ws_listener = tokio::net::TcpListener::bind(ws_addr)
        .await
        .with_context(|| format!("bind WebSocket {ws_addr}"))?;
    tracing::info!(addr = %ws_addr, "WebSocket listener ready");
    write_ws_port(ws_port)?;

    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);

    let mut shutdown = std::pin::pin!(tokio::signal::ctrl_c());

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result.context("accept")?;
                let reg = Arc::clone(&registry);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, reg).await {
                        tracing::warn!(error = %e, "unix connection error");
                    }
                });
            }
            result = ws_listener.accept() => {
                let (stream, _) = result.context("ws accept")?;
                let reg = Arc::clone(&registry);
                let rx = broadcast_tx.subscribe();
                tokio::spawn(async move {
                    if let Err(e) = ws::handle_ws_connection(stream, reg, rx).await {
                        tracing::warn!(error = %e, "ws connection error");
                    }
                });
            }
            _ = &mut shutdown => {
                tracing::info!("shutting down");
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

fn ws_port() -> u16 {
    std::env::var("WEFT_APPD_WS_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7410)
}

fn write_ws_port(port: u16) -> anyhow::Result<()> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR not set")?;
    let path = PathBuf::from(runtime_dir).join("weft/appd.wsport");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, port.to_string())?;
    Ok(())
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    registry: Registry,
) -> anyhow::Result<()> {
    let (reader, writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);
    let mut writer = tokio::io::BufWriter::new(writer);

    while let Some(req) = ipc::read_frame(&mut reader).await? {
        tracing::debug!(?req, "request");
        let resp = dispatch(req, &registry).await;
        ipc::write_frame(&mut writer, &resp).await?;
        writer.flush().await?;
    }
    Ok(())
}

pub(crate) async fn dispatch(req: Request, registry: &Registry) -> Response {
    match req {
        Request::LaunchApp {
            app_id,
            surface_id: _,
        } => {
            let session_id = registry.lock().await.launch(&app_id);
            tracing::info!(session_id, %app_id, "launched");
            Response::LaunchAck { session_id }
        }
        Request::TerminateApp { session_id } => {
            let found = registry.lock().await.terminate(session_id);
            if found {
                tracing::info!(session_id, "terminated");
                Response::AppState {
                    session_id,
                    state: AppStateKind::Stopped,
                }
            } else {
                Response::Error {
                    code: 1,
                    message: format!("session {session_id} not found"),
                }
            }
        }
        Request::QueryRunning => {
            let session_ids = registry.lock().await.running_ids();
            Response::RunningApps { session_ids }
        }
        Request::QueryAppState { session_id } => {
            let state = registry.lock().await.state(session_id);
            Response::AppState { session_id, state }
        }
    }
}

fn appd_socket_path() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("WEFT_APPD_SOCKET") {
        return Ok(PathBuf::from(p));
    }

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR not set")?;

    Ok(PathBuf::from(runtime_dir).join("weft/appd.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc::AppStateKind;

    fn make_registry() -> Registry {
        Arc::new(Mutex::new(SessionRegistry::default()))
    }

    #[tokio::test]
    async fn dispatch_launch_returns_ack() {
        let reg = make_registry();
        let resp = dispatch(
            Request::LaunchApp {
                app_id: "com.test.app".into(),
                surface_id: 0,
            },
            &reg,
        )
        .await;
        match resp {
            Response::LaunchAck { session_id } => assert!(session_id > 0),
            _ => panic!("expected LaunchAck"),
        }
    }

    #[tokio::test]
    async fn dispatch_terminate_known_returns_stopped() {
        let reg = make_registry();
        let ack = dispatch(
            Request::LaunchApp {
                app_id: "app".into(),
                surface_id: 0,
            },
            &reg,
        )
        .await;
        let session_id = match ack {
            Response::LaunchAck { session_id } => session_id,
            _ => panic!("expected LaunchAck"),
        };
        let resp = dispatch(Request::TerminateApp { session_id }, &reg).await;
        assert!(matches!(
            resp,
            Response::AppState {
                state: AppStateKind::Stopped,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn dispatch_terminate_unknown_returns_error() {
        let reg = make_registry();
        let resp = dispatch(Request::TerminateApp { session_id: 999 }, &reg).await;
        assert!(matches!(resp, Response::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_query_running_lists_active_sessions() {
        let reg = make_registry();
        dispatch(
            Request::LaunchApp {
                app_id: "a".into(),
                surface_id: 0,
            },
            &reg,
        )
        .await;
        dispatch(
            Request::LaunchApp {
                app_id: "b".into(),
                surface_id: 0,
            },
            &reg,
        )
        .await;
        let resp = dispatch(Request::QueryRunning, &reg).await;
        match resp {
            Response::RunningApps { session_ids } => assert_eq!(session_ids.len(), 2),
            _ => panic!("expected RunningApps"),
        }
    }

    #[tokio::test]
    async fn dispatch_query_app_state_returns_starting() {
        let reg = make_registry();
        let ack = dispatch(
            Request::LaunchApp {
                app_id: "app".into(),
                surface_id: 0,
            },
            &reg,
        )
        .await;
        let session_id = match ack {
            Response::LaunchAck { session_id } => session_id,
            _ => panic!(),
        };
        let resp = dispatch(Request::QueryAppState { session_id }, &reg).await;
        assert!(matches!(
            resp,
            Response::AppState {
                state: AppStateKind::Starting,
                ..
            }
        ));
    }

    #[test]
    fn registry_launch_increments_id() {
        let mut reg = SessionRegistry::default();
        let id1 = reg.launch("com.example.a");
        let id2 = reg.launch("com.example.b");
        assert!(id2 > id1);
    }

    #[test]
    fn registry_terminate_known_session() {
        let mut reg = SessionRegistry::default();
        let id = reg.launch("com.example.app");
        assert!(reg.terminate(id));
        assert!(matches!(reg.state(id), AppStateKind::NotFound));
    }

    #[test]
    fn registry_terminate_unknown_returns_false() {
        let mut reg = SessionRegistry::default();
        assert!(!reg.terminate(999));
    }

    #[test]
    fn registry_running_ids_reflects_live_sessions() {
        let mut reg = SessionRegistry::default();
        let id1 = reg.launch("a");
        let id2 = reg.launch("b");
        let mut ids = reg.running_ids();
        ids.sort();
        assert_eq!(ids, vec![id1, id2]);
        reg.terminate(id1);
        assert_eq!(reg.running_ids(), vec![id2]);
    }

    #[test]
    fn registry_state_not_found_for_unknown() {
        let reg = SessionRegistry::default();
        assert!(matches!(reg.state(42), AppStateKind::NotFound));
    }
}
