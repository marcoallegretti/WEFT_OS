use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

mod ipc;
mod runtime;
mod ws;

use ipc::{AppStateKind, Request, Response, SessionInfo};

pub(crate) type Registry = Arc<Mutex<SessionRegistry>>;

struct SessionEntry {
    app_id: String,
    state: AppStateKind,
}

struct SessionRegistry {
    next_id: u64,
    sessions: std::collections::HashMap<u64, SessionEntry>,
    broadcast: tokio::sync::broadcast::Sender<Response>,
    abort_senders: std::collections::HashMap<u64, tokio::sync::oneshot::Sender<()>>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        let (broadcast, _) = tokio::sync::broadcast::channel(16);
        Self {
            next_id: 0,
            sessions: std::collections::HashMap::new(),
            broadcast,
            abort_senders: std::collections::HashMap::new(),
        }
    }
}

impl SessionRegistry {
    fn launch(&mut self, app_id: &str) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.sessions.insert(
            id,
            SessionEntry {
                app_id: app_id.to_owned(),
                state: AppStateKind::Starting,
            },
        );
        id
    }

    fn terminate(&mut self, session_id: u64) -> bool {
        let found = self.sessions.remove(&session_id).is_some();
        self.abort_senders.remove(&session_id);
        found
    }

    pub(crate) fn register_abort(&mut self, session_id: u64) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.abort_senders.insert(session_id, tx);
        rx
    }

    fn running_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|(&session_id, e)| SessionInfo {
                session_id,
                app_id: e.app_id.clone(),
            })
            .collect()
    }

    fn state(&self, session_id: u64) -> AppStateKind {
        self.sessions
            .get(&session_id)
            .map(|e| e.state.clone())
            .unwrap_or(AppStateKind::NotFound)
    }

    pub(crate) fn set_state(&mut self, session_id: u64, state: AppStateKind) {
        if let Some(entry) = self.sessions.get_mut(&session_id) {
            entry.state = state;
        }
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Response> {
        self.broadcast.subscribe()
    }

    pub(crate) fn broadcast(&self) -> &tokio::sync::broadcast::Sender<Response> {
        &self.broadcast
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
                let rx = registry.lock().await.subscribe();
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
            let abort_rx = registry.lock().await.register_abort(session_id);
            let reg = Arc::clone(registry);
            let aid = app_id.clone();
            tokio::spawn(async move {
                if let Err(e) = runtime::supervise(session_id, &aid, reg, abort_rx).await {
                    tracing::warn!(session_id, error = %e, "runtime supervisor error");
                }
            });
            Response::LaunchAck { session_id, app_id }
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
            let sessions = registry.lock().await.running_sessions();
            Response::RunningApps { sessions }
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
            Response::LaunchAck {
                session_id,
                ref app_id,
            } => {
                assert!(session_id > 0);
                assert_eq!(app_id, "com.test.app");
            }
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
            Response::LaunchAck { session_id, .. } => session_id,
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
            Response::RunningApps { sessions } => {
                assert_eq!(sessions.len(), 2);
                let mut ids: Vec<&str> = sessions.iter().map(|s| s.app_id.as_str()).collect();
                ids.sort();
                assert_eq!(ids, vec!["a", "b"]);
            }
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
            Response::LaunchAck { session_id, .. } => session_id,
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
    fn registry_running_sessions_reflects_live_sessions() {
        let mut reg = SessionRegistry::default();
        let id1 = reg.launch("com.example.a");
        let id2 = reg.launch("com.example.b");
        let mut sessions = reg.running_sessions();
        sessions.sort_by_key(|s| s.session_id);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, id1);
        assert_eq!(sessions[0].app_id, "com.example.a");
        assert_eq!(sessions[1].session_id, id2);
        assert_eq!(sessions[1].app_id, "com.example.b");
        reg.terminate(id1);
        assert_eq!(reg.running_sessions().len(), 1);
        assert_eq!(reg.running_sessions()[0].session_id, id2);
    }

    #[test]
    fn registry_state_not_found_for_unknown() {
        let reg = SessionRegistry::default();
        assert!(matches!(reg.state(42), AppStateKind::NotFound));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_transitions_through_ready_to_stopped() {
        use std::os::unix::fs::PermissionsExt;

        let script =
            std::env::temp_dir().join(format!("weft_test_runtime_{}.sh", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\necho READY\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let prior = std::env::var("WEFT_RUNTIME_BIN").ok();
        // SAFETY: single-threaded test (flavor = "current_thread"); no concurrent env access.
        unsafe { std::env::set_var("WEFT_RUNTIME_BIN", &script) };

        let registry: Registry = Arc::new(Mutex::new(SessionRegistry::default()));
        let mut rx = registry.lock().await.subscribe();
        let session_id = registry.lock().await.launch("test.app");
        let abort_rx = registry.lock().await.register_abort(session_id);

        runtime::supervise(session_id, "test.app", Arc::clone(&registry), abort_rx)
            .await
            .unwrap();

        assert!(matches!(
            registry.lock().await.state(session_id),
            AppStateKind::Stopped
        ));

        let notification = rx.try_recv();
        assert!(matches!(
            notification,
            Ok(Response::AppReady { session_id: sid }) if sid == session_id
        ));

        let _ = std::fs::remove_file(&script);
        // SAFETY: single-threaded test; restoring env to prior state.
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_RUNTIME_BIN", v),
                None => std::env::remove_var("WEFT_RUNTIME_BIN"),
            }
        }
    }
}
