use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

mod compositor_client;
mod ipc;
mod mount;
mod runtime;
mod ws;

use ipc::{AppInfo, AppStateKind, Request, Response, SessionInfo};

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
    compositor_tx: Option<compositor_client::CompositorSender>,
    ipc_senders: std::collections::HashMap<u64, tokio::sync::mpsc::Sender<String>>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        let (broadcast, _) = tokio::sync::broadcast::channel(16);
        Self {
            next_id: 0,
            sessions: std::collections::HashMap::new(),
            broadcast,
            abort_senders: std::collections::HashMap::new(),
            compositor_tx: None,
            ipc_senders: std::collections::HashMap::new(),
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
            .filter(|(_, e)| !matches!(e.state, AppStateKind::Stopped))
            .map(|(&session_id, e)| SessionInfo {
                session_id,
                app_id: e.app_id.clone(),
            })
            .collect()
    }

    fn running_app_ids(&self) -> Vec<String> {
        self.sessions
            .values()
            .filter(|e| !matches!(e.state, AppStateKind::Stopped))
            .map(|e| e.app_id.clone())
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

    pub(crate) fn remove_abort_sender(&mut self, session_id: u64) {
        self.abort_senders.remove(&session_id);
    }

    pub(crate) fn register_ipc_sender(
        &mut self,
        session_id: u64,
        tx: tokio::sync::mpsc::Sender<String>,
    ) {
        self.ipc_senders.insert(session_id, tx);
    }

    pub(crate) fn ipc_sender_for(
        &self,
        session_id: u64,
    ) -> Option<tokio::sync::mpsc::Sender<String>> {
        self.ipc_senders.get(&session_id).cloned()
    }

    pub(crate) fn remove_ipc_sender(&mut self, session_id: u64) {
        self.ipc_senders.remove(&session_id);
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Response> {
        self.broadcast.subscribe()
    }

    pub(crate) fn broadcast(&self) -> &tokio::sync::broadcast::Sender<Response> {
        &self.broadcast
    }

    pub(crate) fn shutdown_all(&mut self) {
        self.abort_senders.clear();
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
    if let Some(path) = compositor_client::socket_path() {
        let tx = compositor_client::spawn(path);
        registry.lock().await.compositor_tx = Some(tx);
    }

    let ws_port = ws_port();
    let ws_addr: std::net::SocketAddr = format!("127.0.0.1:{ws_port}").parse()?;
    let ws_listener = tokio::net::TcpListener::bind(ws_addr)
        .await
        .with_context(|| format!("bind WebSocket {ws_addr}"))?;
    let ws_bound_port = ws_listener.local_addr()?.port();
    tracing::info!(port = ws_bound_port, "WebSocket listener ready");
    if let Err(e) = write_ws_port(ws_bound_port) {
        tracing::warn!(error = %e, "could not write appd.wsport; servo-shell port discovery will fall back to default");
    }

    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);

    if let Some(app_ids) = load_session() {
        tracing::info!(count = app_ids.len(), "restoring previous session");
        for app_id in app_ids {
            let _ = dispatch(
                crate::ipc::Request::LaunchApp {
                    app_id,
                    surface_id: 0,
                },
                &registry,
            )
            .await;
        }
    }

    #[cfg(unix)]
    let mut sigterm = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate()).context("SIGTERM handler")?
    };

    loop {
        #[cfg(unix)]
        let term = sigterm.recv();
        #[cfg(not(unix))]
        let term = std::future::pending::<Option<()>>();

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
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("SIGINT received; shutting down");
                break;
            }
            _ = term => {
                tracing::info!("SIGTERM received; shutting down");
                break;
            }
        }
    }

    save_session(registry.lock().await.running_app_ids()).await;
    registry.lock().await.shutdown_all();
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

fn session_file_path() -> Option<std::path::PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(PathBuf::from(runtime_dir).join("weft/last-session.json"))
}

async fn save_session(app_ids: Vec<String>) {
    let Some(path) = session_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&app_ids) {
        let _ = std::fs::write(&path, json);
    }
}

fn load_session() -> Option<Vec<String>> {
    let path = session_file_path()?;
    let json = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&json).ok()
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
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&path, port.to_string()).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    registry: Registry,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let cred = stream.peer_cred().context("SO_PEERCRED")?;
        let our_uid = unsafe { libc::getuid() };
        if cred.uid() != our_uid {
            anyhow::bail!(
                "peer UID {} != process UID {}; connection rejected",
                cred.uid(),
                our_uid
            );
        }
    }

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

pub(crate) fn session_ipc_socket_path(session_id: u64) -> Option<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let dir = PathBuf::from(runtime_dir).join("weft");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("ipc-{session_id}.sock")))
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
            let compositor_tx = registry.lock().await.compositor_tx.clone();
            let ipc_socket = session_ipc_socket_path(session_id);
            let broadcast = registry.lock().await.broadcast().clone();
            if let Some(ref sock_path) = ipc_socket {
                if let Some(tx) =
                    runtime::spawn_ipc_relay(session_id, sock_path.clone(), broadcast).await
                {
                    registry.lock().await.register_ipc_sender(session_id, tx);
                }
            }
            let reg = Arc::clone(registry);
            let aid = app_id.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    runtime::supervise(session_id, &aid, reg, abort_rx, compositor_tx, ipc_socket)
                        .await
                {
                    tracing::warn!(session_id, error = %e, "runtime supervisor error");
                }
            });
            let _ = registry.lock().await.broadcast().send(Response::LaunchAck {
                session_id,
                app_id: app_id.clone(),
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
        Request::QueryInstalledApps => {
            let apps = scan_installed_apps();
            Response::InstalledApps { apps }
        }
        Request::IpcForward {
            session_id,
            payload,
        } => {
            if let Some(tx) = registry.lock().await.ipc_sender_for(session_id) {
                if tx.send(payload).await.is_err() {
                    tracing::warn!(session_id, "IPC relay sender closed");
                    registry.lock().await.remove_ipc_sender(session_id);
                }
            }
            Response::AppState {
                session_id,
                state: ipc::AppStateKind::Running,
            }
        }
        Request::PanelGesture {
            gesture_type,
            fingers,
            dx,
            dy,
        } => {
            let msg = Response::NavigationGesture {
                gesture_type,
                fingers,
                dx,
                dy,
            };
            let _ = registry.lock().await.broadcast().send(msg.clone());
            msg
        }
    }
}

pub(crate) fn app_store_roots() -> Vec<std::path::PathBuf> {
    if let Ok(explicit) = std::env::var("WEFT_APP_STORE") {
        return vec![std::path::PathBuf::from(explicit)];
    }
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(
            std::path::PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("weft")
                .join("apps"),
        );
    }
    roots.push(std::path::PathBuf::from("/usr/share/weft/apps"));
    roots
}

#[derive(serde::Deserialize)]
struct WappPackage {
    id: String,
    name: String,
    version: String,
}

#[derive(serde::Deserialize)]
struct WappManifest {
    package: WappPackage,
}

fn scan_installed_apps() -> Vec<AppInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for root in app_store_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("wapp.toml");
            let Ok(contents) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Ok(m) = toml::from_str::<WappManifest>(&contents) else {
                continue;
            };
            if seen.insert(m.package.id.clone()) {
                apps.push(AppInfo {
                    app_id: m.package.id,
                    name: m.package.name,
                    version: m.package.version,
                });
            }
        }
    }
    apps
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

    #[test]
    fn ws_port_defaults_to_7410() {
        let prior = std::env::var("WEFT_APPD_WS_PORT").ok();
        unsafe { std::env::remove_var("WEFT_APPD_WS_PORT") };
        let port = ws_port();
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APPD_WS_PORT", v),
                None => {}
            }
        }
        assert_eq!(port, 7410);
    }

    #[test]
    fn ws_port_uses_env_override() {
        let prior = std::env::var("WEFT_APPD_WS_PORT").ok();
        unsafe { std::env::set_var("WEFT_APPD_WS_PORT", "9000") };
        let port = ws_port();
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APPD_WS_PORT", v),
                None => std::env::remove_var("WEFT_APPD_WS_PORT"),
            }
        }
        assert_eq!(port, 9000);
    }

    #[test]
    fn appd_socket_path_uses_override_env() {
        let prior = std::env::var("WEFT_APPD_SOCKET").ok();
        unsafe { std::env::set_var("WEFT_APPD_SOCKET", "/tmp/custom.sock") };
        let path = appd_socket_path().unwrap();
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APPD_SOCKET", v),
                None => std::env::remove_var("WEFT_APPD_SOCKET"),
            }
        }
        assert_eq!(path, PathBuf::from("/tmp/custom.sock"));
    }

    #[test]
    fn appd_socket_path_errors_without_xdg_and_no_override() {
        let prior_sock = std::env::var("WEFT_APPD_SOCKET").ok();
        let prior_xdg = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe {
            std::env::remove_var("WEFT_APPD_SOCKET");
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
        let result = appd_socket_path();
        unsafe {
            match prior_sock {
                Some(v) => std::env::set_var("WEFT_APPD_SOCKET", v),
                None => {}
            }
            match prior_xdg {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => {}
            }
        }
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn dispatch_launch_returns_ack() {
        let reg = make_registry();
        let mut rx = reg.lock().await.subscribe();
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
        assert!(
            matches!(rx.try_recv(), Ok(Response::LaunchAck { .. })),
            "LaunchAck must also be broadcast"
        );
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
    async fn session_stops_when_runtime_bin_absent() {
        unsafe { std::env::remove_var("WEFT_RUNTIME_BIN") };
        let reg = make_registry();
        let ack = dispatch(
            Request::LaunchApp {
                app_id: "com.test.nobinary".into(),
                surface_id: 0,
            },
            &reg,
        )
        .await;
        let session_id = match ack {
            Response::LaunchAck { session_id, .. } => session_id,
            _ => panic!("expected LaunchAck"),
        };
        tokio::task::yield_now().await;
        let state = reg.lock().await.state(session_id);
        assert!(
            matches!(state, AppStateKind::Stopped),
            "session should be Stopped when WEFT_RUNTIME_BIN is absent"
        );
    }

    #[tokio::test]
    async fn running_sessions_excludes_stopped() {
        let reg = make_registry();
        let session_id = reg.lock().await.launch("com.test.app");
        reg.lock()
            .await
            .set_state(session_id, AppStateKind::Stopped);
        let sessions = reg.lock().await.running_sessions();
        assert!(sessions.is_empty());
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

    #[tokio::test]
    async fn dispatch_query_app_state_unknown_returns_not_found() {
        let reg = make_registry();
        let resp = dispatch(Request::QueryAppState { session_id: 9999 }, &reg).await;
        assert!(matches!(
            resp,
            Response::AppState {
                state: AppStateKind::NotFound,
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

    #[tokio::test]
    async fn dispatch_query_installed_returns_installed_apps() {
        let reg = make_registry();
        let resp = dispatch(Request::QueryInstalledApps, &reg).await;
        assert!(matches!(resp, Response::InstalledApps { .. }));
    }

    #[test]
    fn scan_installed_apps_finds_valid_packages() {
        use std::fs;
        let store = std::env::temp_dir().join(format!("weft_appd_scan_{}", std::process::id()));
        let app_dir = store.join("com.example.scanner");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("wapp.toml"),
            "[package]\nid = \"com.example.scanner\"\nname = \"Scanner\"\nversion = \"1.0.0\"\n\
             [runtime]\nmodule = \"app.wasm\"\n[ui]\nentry = \"ui/index.html\"\n",
        )
        .unwrap();

        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::set_var("WEFT_APP_STORE", &store) };

        let apps = scan_installed_apps();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].app_id, "com.example.scanner");
        assert_eq!(apps[0].name, "Scanner");
        assert_eq!(apps[0].version, "1.0.0");

        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
        }
        let _ = fs::remove_dir_all(&store);
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
        let prior_cgroup = std::env::var("WEFT_DISABLE_CGROUP").ok();
        // SAFETY: single-threaded test (flavor = "current_thread"); no concurrent env access.
        unsafe {
            std::env::set_var("WEFT_RUNTIME_BIN", &script);
            std::env::set_var("WEFT_DISABLE_CGROUP", "1");
        }

        let registry: Registry = Arc::new(Mutex::new(SessionRegistry::default()));
        let mut rx = registry.lock().await.subscribe();
        let session_id = registry.lock().await.launch("test.app");
        let abort_rx = registry.lock().await.register_abort(session_id);

        runtime::supervise(
            session_id,
            "test.app",
            Arc::clone(&registry),
            abort_rx,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            registry.lock().await.state(session_id),
            AppStateKind::Stopped
        ));

        let notification = rx.try_recv();
        assert!(matches!(
            notification,
            Ok(Response::AppReady { session_id: sid, .. }) if sid == session_id
        ));

        let stopped = rx.try_recv();
        assert!(matches!(
            stopped,
            Ok(Response::AppState { session_id: sid, state: AppStateKind::Stopped }) if sid == session_id
        ));

        let _ = std::fs::remove_file(&script);
        // SAFETY: single-threaded test; restoring env to prior state.
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_RUNTIME_BIN", v),
                None => std::env::remove_var("WEFT_RUNTIME_BIN"),
            }
            match prior_cgroup {
                Some(v) => std::env::set_var("WEFT_DISABLE_CGROUP", v),
                None => std::env::remove_var("WEFT_DISABLE_CGROUP"),
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_abort_during_startup_broadcasts_stopped() {
        use std::os::unix::fs::PermissionsExt;

        let script =
            std::env::temp_dir().join(format!("weft_test_sleep_{}.sh", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\nsleep 60\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let prior = std::env::var("WEFT_RUNTIME_BIN").ok();
        unsafe { std::env::set_var("WEFT_RUNTIME_BIN", &script) };

        let registry: Registry = Arc::new(Mutex::new(SessionRegistry::default()));
        let mut rx = registry.lock().await.subscribe();
        let session_id = registry.lock().await.launch("test.abort.startup");
        let abort_rx = registry.lock().await.register_abort(session_id);

        registry.lock().await.terminate(session_id);

        runtime::supervise(
            session_id,
            "test.abort.startup",
            Arc::clone(&registry),
            abort_rx,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            registry.lock().await.state(session_id),
            AppStateKind::NotFound
        ));

        let broadcast = rx.try_recv();
        assert!(matches!(
            broadcast,
            Ok(Response::AppState { session_id: sid, state: AppStateKind::Stopped }) if sid == session_id
        ));

        let _ = std::fs::remove_file(&script);
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_RUNTIME_BIN", v),
                None => std::env::remove_var("WEFT_RUNTIME_BIN"),
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_spawn_failure_broadcasts_stopped() {
        let prior = std::env::var("WEFT_RUNTIME_BIN").ok();
        unsafe {
            std::env::set_var(
                "WEFT_RUNTIME_BIN",
                "/nonexistent/path/to/weft-runtime-does-not-exist",
            )
        };

        let registry: Registry = Arc::new(Mutex::new(SessionRegistry::default()));
        let mut rx = registry.lock().await.subscribe();
        let session_id = registry.lock().await.launch("test.spawn.fail");
        let abort_rx = registry.lock().await.register_abort(session_id);

        runtime::supervise(
            session_id,
            "test.spawn.fail",
            Arc::clone(&registry),
            abort_rx,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            registry.lock().await.state(session_id),
            AppStateKind::Stopped
        ));

        let broadcast = rx.try_recv();
        assert!(matches!(
            broadcast,
            Ok(Response::AppState { session_id: sid, state: AppStateKind::Stopped }) if sid == session_id
        ));

        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_RUNTIME_BIN", v),
                None => std::env::remove_var("WEFT_RUNTIME_BIN"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_save_load_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("weft_session_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prior = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &tmp) };

        let app_ids = vec!["com.example.foo".to_string(), "com.example.bar".to_string()];
        save_session(app_ids.clone()).await;

        let loaded = load_session();
        assert!(loaded.is_some());
        let mut loaded = loaded.unwrap();
        loaded.sort();
        let mut expected = app_ids.clone();
        expected.sort();
        assert_eq!(loaded, expected);

        assert!(load_session().is_none());

        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            match prior {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_save_empty_load_returns_empty_vec() {
        let tmp = std::env::temp_dir().join(format!("weft_session_empty_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prior = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &tmp) };

        save_session(vec![]).await;
        let loaded = load_session();
        assert!(matches!(loaded, Some(v) if v.is_empty()));

        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            match prior {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    #[test]
    fn load_session_no_file_returns_none() {
        let tmp = std::env::temp_dir().join(format!("weft_session_missing_{}", std::process::id()));
        let prior = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &tmp) };

        assert!(load_session().is_none());

        unsafe {
            match prior {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }
}
