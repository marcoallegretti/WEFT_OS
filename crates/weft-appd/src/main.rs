#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
#[cfg(unix)]
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[cfg(unix)]
mod compositor_client;
#[cfg(not(unix))]
mod compositor_client {
    use tokio::sync::mpsc;
    use weft_ipc_types::AppdToCompositor;

    pub type CompositorSender = mpsc::Sender<AppdToCompositor>;
}
mod grants;
mod ipc;
mod launch;
mod mount;
mod runtime;
mod ws;

use ipc::{AppInfo, AppStateKind, Request, Response, SessionInfo};

pub(crate) type Registry = Arc<Mutex<SessionRegistry>>;

struct SessionEntry {
    app_id: String,
    state: AppStateKind,
    /// Credential that binds application bridge connections to this session.
    bridge_token: Option<String>,
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
                bridge_token: match runtime::random_token() {
                    Ok(token) => Some(token),
                    Err(e) => {
                        tracing::error!(session_id = id, error = %e, "no bridge token; app messages disabled");
                        None
                    }
                },
            },
        );
        id
    }

    pub(crate) fn bridge_token(&self, session_id: u64) -> Option<String> {
        self.sessions.get(&session_id)?.bridge_token.clone()
    }

    /// Whether `token` authorizes an application bridge for a live session.
    pub(crate) fn authorize_bridge(&self, session_id: u64, token: &str) -> bool {
        self.sessions.get(&session_id).is_some_and(|entry| {
            !matches!(entry.state, AppStateKind::Stopped)
                && entry
                    .bridge_token
                    .as_deref()
                    .is_some_and(|expected| ws::tokens_match(expected, token))
        })
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

#[cfg(unix)]
async fn run() -> anyhow::Result<()> {
    let socket_path = appd_socket_path()?;

    if let Some(parent) = socket_path.parent() {
        private_dir(parent).with_context(|| format!("create {}", parent.display()))?;
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
    let ws_auth = Arc::new(ws::WsAuth {
        system_token: runtime::random_token().context("create system token")?,
    });
    write_system_token(&ws_auth.system_token).context("write appd.systoken")?;

    #[cfg(unix)]
    {
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
    }

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
                let auth = Arc::clone(&ws_auth);
                tokio::spawn(async move {
                    if let Err(e) = ws::handle_ws_connection(stream, reg, rx, auth).await {
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
    // A newer appd may already have replaced the file; only this instance's
    // own token is removed.
    if let Ok(path) = system_token_path()
        && std::fs::read_to_string(&path).is_ok_and(|t| t == ws_auth.system_token)
    {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

#[cfg(not(unix))]
async fn run() -> anyhow::Result<()> {
    anyhow::bail!("weft-appd requires a Unix platform")
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn session_file_path() -> Option<std::path::PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(PathBuf::from(runtime_dir).join("weft/last-session.json"))
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
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

/// Writes the system-role WebSocket credential to a file only the user can read.
#[cfg_attr(not(unix), allow(dead_code))]
fn write_system_token(token: &str) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let path = system_token_path()?;
    let staging = path.with_extension(format!("systoken.{}", std::process::id()));
    // A leftover staging file could carry other permissions; the file is
    // always created afresh so the mode below applies.
    let _ = std::fs::remove_file(&staging);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let written = options.open(&staging).and_then(|mut file| {
        file.write_all(token.as_bytes())?;
        file.sync_all()
    });
    let result = written.and_then(|()| std::fs::rename(&staging, &path));
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result
}

fn system_token_path() -> std::io::Result<PathBuf> {
    let dir = PathBuf::from(
        std::env::var("XDG_RUNTIME_DIR")
            .map_err(|_| std::io::Error::other("XDG_RUNTIME_DIR not set"))?,
    )
    .join("weft");
    private_dir(&dir)?;
    Ok(dir.join("appd.systoken"))
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn write_ws_port(port: u16) -> anyhow::Result<()> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR not set")?;
    let path = PathBuf::from(runtime_dir).join("weft/appd.wsport");
    if let Some(parent) = path.parent() {
        private_dir(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&path, port.to_string()).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
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
    private_dir(&dir).ok()?;
    Some(dir.join(format!("ipc-{session_id}.sock")))
}

/// Creates appd's runtime directory (`$XDG_RUNTIME_DIR/weft`) or another of
/// its directories, accessible to the user only whatever the umask, also
/// when it already existed.
pub(crate) fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) async fn dispatch(req: Request, registry: &Registry) -> Response {
    match req {
        Request::LaunchApp {
            app_id,
            surface_id: _,
        } => {
            // The package is resolved and its capabilities granted before a
            // session exists; a package this host cannot satisfy is refused,
            // not started.
            let launch = if !weft_ipc_types::package::is_valid_app_id(&app_id) {
                Err(grants::Refusal::new(400, "invalid app ID"))
            } else if std::env::var("WEFT_RUNTIME_BIN").is_ok() {
                resolve_launch(app_id.clone()).await.map(Some)
            } else {
                Ok(None)
            };
            let (package, grants) = match launch {
                Ok(Some((package, grants))) => (Some(package), grants),
                Ok(None) => (None, grants::SessionGrants::default()),
                Err(refusal) => {
                    tracing::warn!(%app_id, error = %refusal.message, "launch refused");
                    return Response::Error {
                        code: refusal.code,
                        message: format!("{app_id}: {}", refusal.message),
                    };
                }
            };
            let session_id = registry.lock().await.launch(&app_id);
            tracing::info!(session_id, %app_id, "launched");
            let abort_rx = registry.lock().await.register_abort(session_id);
            let compositor_tx = registry.lock().await.compositor_tx.clone();

            // Without a configured runtime, nothing was resolved and the
            // session stops at once.
            let Some(package) = package else {
                let _ = registry.lock().await.broadcast().send(Response::LaunchAck {
                    session_id,
                    app_id: app_id.clone(),
                });
                {
                    let mut reg = registry.lock().await;
                    reg.set_state(session_id, AppStateKind::Stopped);
                    reg.remove_abort_sender(session_id);
                }
                let _ = registry.lock().await.broadcast().send(Response::AppState {
                    session_id,
                    state: AppStateKind::Stopped,
                });
                return Response::LaunchAck { session_id, app_id };
            };
            let reg = Arc::clone(registry);
            tokio::spawn(async move {
                if let Err(e) =
                    runtime::supervise(session_id, package, grants, reg, abort_rx, compositor_tx)
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
            // The registry lock is released before sending; a component that
            // stops reading must not stall appd.
            let sender = registry.lock().await.ipc_sender_for(session_id);
            let Some(tx) = sender else {
                return Response::Error {
                    code: 1,
                    message: format!("session {session_id} has no IPC relay"),
                };
            };
            match tx.try_send(payload) {
                Ok(()) => Response::AppState {
                    session_id,
                    state: ipc::AppStateKind::Running,
                },
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Response::Error {
                    code: 1,
                    message: format!("session {session_id} is not reading messages"),
                },
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    registry.lock().await.remove_ipc_sender(session_id);
                    Response::Error {
                        code: 1,
                        message: format!("session {session_id} has no IPC relay"),
                    }
                }
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

/// Resolves the package of `app_id` and derives its grants. Resolution may
/// mount an image, so it runs off the async workers; a refused launch
/// releases the mount the same way.
async fn resolve_launch(
    app_id: String,
) -> Result<(launch::LaunchPackage, grants::SessionGrants), grants::Refusal> {
    tokio::task::spawn_blocking(move || {
        let package = launch::resolve(&app_id)?;
        let grants = grants::derive(&app_id, &package.capabilities, grants::HostDirs::from_env)?;
        tracing::info!(
            %app_id,
            version = %package.version,
            root = %package.root().display(),
            image = package.image.is_some(),
            "package resolved"
        );
        Ok((package, grants))
    })
    .await
    .unwrap_or_else(|e| {
        Err(grants::Refusal::new(
            500,
            format!("package resolution failed: {e}"),
        ))
    })
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

fn scan_installed_apps() -> Vec<AppInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for root in app_store_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(m) = weft_ipc_types::manifest::Manifest::read(&entry.path()) else {
                continue;
            };
            // Only a directory named after the ID its manifest declares is an
            // installed package; staging copies and stray directories are not.
            if entry.file_name().to_str() != Some(m.package.id.as_str()) {
                continue;
            }
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

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
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

    static ENV_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

    /// Points XDG_RUNTIME_DIR at a directory private to this test process,
    /// since session sockets are named by session ID and every test registry
    /// starts at 1. Directories of test processes that have ended are
    /// removed. Callers hold env_lock.
    pub(crate) fn use_test_runtime_dir() {
        let base = std::env::temp_dir().join("weft-appd-tests");
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let alive = entry
                    .file_name()
                    .to_str()
                    .and_then(|pid| pid.parse::<u32>().ok())
                    .is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
                if !alive {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let dir = base.join(std::process::id().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: env_lock serialises the tests that change the environment,
        // and std serialises the environment accesses themselves.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };
    }

    pub(crate) fn env_lock() -> &'static tokio::sync::Mutex<()> {
        ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Writes a complete package `id` into `dir`; `extra` adds lines to its
    /// `[package]` table.
    fn write_test_package(dir: &std::path::Path, id: &str, extra: &str) {
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        std::fs::write(dir.join("ui/index.html"), b"<p>").unwrap();
        std::fs::write(
            dir.join("wapp.toml"),
            format!(
                "[package]\nid = \"{id}\"\nname = \"T\"\nversion = \"0.1.0\"\n{extra}\n\
                 [runtime]\nmodule = \"app.wasm\"\n[ui]\nentry = \"ui/index.html\"\n"
            ),
        )
        .unwrap();
        record_development(id);
    }

    /// Records `id` as development content in a data home private to this
    /// test process, which XDG_DATA_HOME then points at, with an empty trust
    /// store so the host's keys never affect a test. Data homes left by
    /// earlier test processes are removed. Callers hold env_lock.
    fn record_development(id: &str) {
        let name = format!("weft-appd-tests-data-{}", std::process::id());
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
            for entry in entries.flatten() {
                // Only data homes of test processes that have exited.
                let stale = entry
                    .file_name()
                    .to_str()
                    .filter(|n| *n != name)
                    .and_then(|n| n.strip_prefix("weft-appd-tests-data-"))
                    .is_some_and(|pid| !std::path::Path::new("/proc").join(pid).exists());
                if stale {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let data_home = std::env::temp_dir().join(name);
        let keys = data_home.join("trusted-keys");
        std::fs::create_dir_all(&keys).unwrap();
        let record = weft_ipc_types::trust::owner_record_path(&data_home, id);
        if !record.exists() {
            weft_ipc_types::trust::write_owner(&record, weft_ipc_types::trust::Owner::Development)
                .unwrap();
        }
        // SAFETY: callers hold env_lock, which serialises environment changes.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("WEFT_TRUSTED_KEYS", &keys);
        }
    }

    fn make_registry() -> Registry {
        Arc::new(Mutex::new(SessionRegistry::default()))
    }

    #[tokio::test]
    async fn ws_port_defaults_to_7410() {
        let _env = env_lock().lock().await;
        let prior = std::env::var("WEFT_APPD_WS_PORT").ok();
        unsafe { std::env::remove_var("WEFT_APPD_WS_PORT") };
        let port = ws_port();
        unsafe {
            if let Some(v) = prior {
                std::env::set_var("WEFT_APPD_WS_PORT", v)
            }
        }
        assert_eq!(port, 7410);
    }

    #[tokio::test]
    async fn ws_port_uses_env_override() {
        let _env = env_lock().lock().await;
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
        let _env = env_lock().blocking_lock();
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
        let _env = env_lock().blocking_lock();
        let prior_sock = std::env::var("WEFT_APPD_SOCKET").ok();
        let prior_xdg = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe {
            std::env::remove_var("WEFT_APPD_SOCKET");
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
        let result = appd_socket_path();
        unsafe {
            if let Some(v) = prior_sock {
                std::env::set_var("WEFT_APPD_SOCKET", v)
            }
            if let Some(v) = prior_xdg {
                std::env::set_var("XDG_RUNTIME_DIR", v)
            }
        }
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn launch_is_refused_when_capabilities_cannot_be_granted() {
        let _env = env_lock().lock().await;
        let store = std::env::temp_dir().join(format!("weft_refuse_{}", std::process::id()));
        let app_dir = store.join("org.example.gpu");
        write_test_package(
            &app_dir,
            "org.example.gpu",
            "capabilities = [\"hw:gpu:compute\"]",
        );
        let prior_store = std::env::var("WEFT_APP_STORE").ok();
        let prior_bin = std::env::var("WEFT_RUNTIME_BIN").ok();
        // SAFETY: env_lock is held and the runtime is current_thread.
        unsafe {
            std::env::set_var("WEFT_APP_STORE", &store);
            std::env::set_var("WEFT_RUNTIME_BIN", "/nonexistent/weft-runtime");
        }

        let reg = make_registry();
        let resp = dispatch(
            Request::LaunchApp {
                app_id: "org.example.gpu".into(),
                surface_id: 0,
            },
            &reg,
        )
        .await;

        // SAFETY: as above.
        unsafe {
            match prior_store {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
            match prior_bin {
                Some(v) => std::env::set_var("WEFT_RUNTIME_BIN", v),
                None => std::env::remove_var("WEFT_RUNTIME_BIN"),
            }
        }
        let _ = std::fs::remove_dir_all(&store);

        match resp {
            Response::Error { code, message } => {
                assert_eq!(code, 403);
                assert!(message.contains("hw:gpu:compute"), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(reg.lock().await.running_sessions().is_empty());
        assert!(matches!(reg.lock().await.state(1), AppStateKind::NotFound));
    }

    #[tokio::test]
    async fn dispatch_launch_returns_ack() {
        let _env = env_lock().lock().await;
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
        let _env = env_lock().lock().await;
        let reg = make_registry();
        let ack = dispatch(
            Request::LaunchApp {
                app_id: "com.test.app".into(),
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

    #[tokio::test(flavor = "current_thread")]
    async fn session_stops_when_runtime_bin_absent() {
        let _env = env_lock().lock().await;
        let prior = std::env::var("WEFT_RUNTIME_BIN").ok();
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
        let mut stopped = false;
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if matches!(reg.lock().await.state(session_id), AppStateKind::Stopped) {
                stopped = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let state = reg.lock().await.state(session_id);
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_RUNTIME_BIN", v),
                None => std::env::remove_var("WEFT_RUNTIME_BIN"),
            }
        }
        assert!(
            stopped && matches!(state, AppStateKind::Stopped),
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
        reg.lock().await.launch("a");
        reg.lock().await.launch("b");
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
        let session_id = reg.lock().await.launch("app");
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

        let _env = env_lock().blocking_lock();
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
    #[tokio::test]
    async fn ipc_forward_never_waits_on_a_component() {
        let registry = make_registry();
        let forward = |session_id| Request::IpcForward {
            session_id,
            payload: "x".into(),
        };
        let reply = dispatch(forward(7), &registry).await;
        assert!(matches!(reply, Response::Error { .. }), "{reply:?}");

        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        registry.lock().await.register_ipc_sender(7, tx);
        assert!(matches!(
            dispatch(forward(7), &registry).await,
            Response::AppState { .. }
        ));
        // The queue is full and nothing reads it: the reply comes at once.
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            dispatch(forward(7), &registry),
        )
        .await
        .expect("IPC_FORWARD blocked");
        assert!(matches!(reply, Response::Error { .. }), "{reply:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_closed_ipc_connection_ends_the_session() {
        use std::os::unix::fs::PermissionsExt;
        let _env = env_lock().lock().await;
        use_test_runtime_dir();
        let dir = std::env::temp_dir().join(format!("weft_test_ipc_close_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Both children report ready and keep running; the test plays the
        // component, connecting to the session's IPC socket and closing it.
        let child = dir.join("child.sh");
        std::fs::write(
            &child,
            "#!/bin/sh\necho READY $WEFT_READY_TOKEN\nexec sleep 30\n",
        )
        .unwrap();
        std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o755)).unwrap();
        let prior: Vec<_> = [
            "WEFT_RUNTIME_BIN",
            "WEFT_APP_SHELL_BIN",
            "WEFT_DISABLE_CGROUP",
        ]
        .iter()
        .map(|k| (*k, std::env::var_os(k)))
        .collect();
        // SAFETY: env_lock serialises the tests that change the environment.
        unsafe {
            std::env::set_var("WEFT_RUNTIME_BIN", &child);
            std::env::set_var("WEFT_APP_SHELL_BIN", &child);
            std::env::set_var("WEFT_DISABLE_CGROUP", "1");
        }

        let registry = make_registry();
        let mut rx = registry.lock().await.subscribe();
        let session_id = registry.lock().await.launch("test.app");
        let abort_rx = registry.lock().await.register_abort(session_id);
        let socket = session_ipc_socket_path(session_id).unwrap();
        let component = async {
            // Connect once the session is running, then close the connection.
            loop {
                if let Ok(Response::AppReady { session_id: id, .. }) = rx.recv().await
                    && id == session_id
                {
                    break;
                }
            }
            drop(tokio::net::UnixStream::connect(&socket).await.unwrap());
        };
        let supervised = runtime::supervise(
            session_id,
            launch::LaunchPackage::unresolved("test.app"),
            grants::SessionGrants::default(),
            Arc::clone(&registry),
            abort_rx,
            None,
        );
        let finished = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            tokio::join!(supervised, component)
        })
        .await;

        for (key, value) in prior {
            // SAFETY: as above.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(finished.is_ok(), "the session outlived its IPC connection");
        assert!(matches!(
            registry.lock().await.state(session_id),
            AppStateKind::Stopped
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_stopped_session_releases_its_ipc_relay() {
        use std::os::unix::fs::PermissionsExt;
        let _env = env_lock().lock().await;
        use_test_runtime_dir();
        let dir = std::env::temp_dir().join(format!("weft_test_relay_{}", std::process::id()));
        let app = dir.join("store/org.example.relay");
        write_test_package(&app, "org.example.relay", "");
        // The runtime reports ready and exits without connecting to the relay.
        let child = dir.join("child.sh");
        std::fs::write(
            &child,
            "#!/bin/sh\necho READY $WEFT_READY_TOKEN\nexec sleep 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vars = [
            ("WEFT_RUNTIME_BIN", child.clone().into_os_string()),
            ("WEFT_APP_SHELL_BIN", child.clone().into_os_string()),
            ("WEFT_DISABLE_CGROUP", "1".into()),
            ("WEFT_APP_STORE", dir.join("store").into_os_string()),
        ];
        let prior: Vec<_> = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (key, value) in &vars {
            // SAFETY: env_lock is held on a current_thread runtime.
            unsafe { std::env::set_var(key, value) };
        }

        let registry = make_registry();
        let mut rx = registry.lock().await.subscribe();
        let ack = dispatch(
            Request::LaunchApp {
                app_id: "org.example.relay".into(),
                surface_id: 0,
            },
            &registry,
        )
        .await;
        let Response::LaunchAck { session_id, .. } = ack else {
            panic!("expected LaunchAck, got {ack:?}");
        };
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Ok(Response::AppState {
                    session_id: id,
                    state: AppStateKind::Stopped,
                }) = rx.recv().await
                    && id == session_id
                {
                    break;
                }
            }
        })
        .await;

        for (key, value) in prior {
            // SAFETY: as above.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(stopped.is_ok(), "session did not stop");
        assert!(registry.lock().await.ipc_sender_for(session_id).is_none());
        assert!(!session_ipc_socket_path(session_id).unwrap().exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn children_receive_the_resolved_package_files() {
        use std::os::unix::fs::PermissionsExt;
        let _env = env_lock().lock().await;
        use_test_runtime_dir();
        let dir = std::env::temp_dir().join(format!("weft_test_args_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = dir.join("store/org.example.args");
        write_test_package(&app, "org.example.args", "");
        // Each child records its arguments, one per line, then reports ready.
        let log = dir.join("args.log");
        let child = dir.join("child.sh");
        std::fs::write(
            &child,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" -- >> '{}'\necho READY $WEFT_READY_TOKEN\nexec sleep 1\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vars = [
            ("WEFT_RUNTIME_BIN", child.clone().into_os_string()),
            ("WEFT_APP_SHELL_BIN", child.clone().into_os_string()),
            ("WEFT_DISABLE_CGROUP", "1".into()),
            ("WEFT_APP_STORE", dir.join("store").into_os_string()),
        ];
        let prior: Vec<_> = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (key, value) in &vars {
            // SAFETY: env_lock is held on a current_thread runtime.
            unsafe { std::env::set_var(key, value) };
        }

        let registry = make_registry();
        let mut rx = registry.lock().await.subscribe();
        let ack = dispatch(
            Request::LaunchApp {
                app_id: "org.example.args".into(),
                surface_id: 0,
            },
            &registry,
        )
        .await;
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !matches!(
                rx.recv().await,
                Ok(Response::AppState {
                    state: AppStateKind::Stopped,
                    ..
                })
            ) {}
        })
        .await;
        let recorded = std::fs::read_to_string(&log).unwrap_or_default();

        for (key, value) in prior {
            // SAFETY: as above.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(ack, Response::LaunchAck { .. }), "{ack:?}");
        assert!(stopped.is_ok(), "session did not stop");
        let runs: Vec<Vec<&str>> = recorded
            .split("--\n")
            .filter(|run| !run.is_empty())
            .map(|run| run.lines().collect())
            .collect();
        let after = |run: &[&str], flag: &str| {
            run.iter()
                .position(|arg| *arg == flag)
                .and_then(|i| run.get(i + 1).map(|value| value.to_string()))
        };
        let module = app.join("app.wasm").display().to_string();
        let ui = app.join("ui/index.html").display().to_string();
        assert_eq!(runs.len(), 2, "{recorded}");
        assert_eq!(after(&runs[0], "--module"), Some(module), "{recorded}");
        assert_eq!(after(&runs[1], "--ui"), Some(ui), "{recorded}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_transitions_through_ready_to_stopped() {
        use std::os::unix::fs::PermissionsExt;

        let _env = env_lock().lock().await;
        let script =
            std::env::temp_dir().join(format!("weft_test_runtime_{}.sh", std::process::id()));
        std::fs::write(
            &script,
            "#!/bin/sh\necho READY $WEFT_READY_TOKEN\nexec sleep 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let shell = std::env::temp_dir().join(format!("weft_test_shell_{}.sh", std::process::id()));
        std::fs::write(
            &shell,
            "#!/bin/sh\necho READY $WEFT_READY_TOKEN\nexec sleep 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();

        let prior = std::env::var("WEFT_RUNTIME_BIN").ok();
        let prior_shell = std::env::var("WEFT_APP_SHELL_BIN").ok();
        let prior_cgroup = std::env::var("WEFT_DISABLE_CGROUP").ok();
        // SAFETY: single-threaded test (flavor = "current_thread"); no concurrent env access.
        unsafe {
            std::env::set_var("WEFT_RUNTIME_BIN", &script);
            std::env::set_var("WEFT_APP_SHELL_BIN", &shell);
            std::env::set_var("WEFT_DISABLE_CGROUP", "1");
        }

        let registry: Registry = Arc::new(Mutex::new(SessionRegistry::default()));
        let mut rx = registry.lock().await.subscribe();
        let session_id = registry.lock().await.launch("test.app");
        let abort_rx = registry.lock().await.register_abort(session_id);

        use_test_runtime_dir();

        runtime::supervise(
            session_id,
            launch::LaunchPackage::unresolved("test.app"),
            grants::SessionGrants::default(),
            Arc::clone(&registry),
            abort_rx,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            registry.lock().await.state(session_id),
            AppStateKind::Stopped
        ));
        // The session's IPC relay ends with it, although the test runtime
        // never connected to it.
        assert!(registry.lock().await.ipc_sender_for(session_id).is_none());
        assert!(!session_ipc_socket_path(session_id).unwrap().exists());

        let notification =
            tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(matches!(
            notification,
            Ok(Ok(Response::AppReady { session_id: sid, .. })) if sid == session_id
        ));

        let stopped = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(matches!(
            stopped,
            Ok(Ok(Response::AppState { session_id: sid, state: AppStateKind::Stopped })) if sid == session_id
        ));

        let _ = std::fs::remove_file(&script);
        let _ = std::fs::remove_file(&shell);
        // SAFETY: single-threaded test; restoring env to prior state.
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_RUNTIME_BIN", v),
                None => std::env::remove_var("WEFT_RUNTIME_BIN"),
            }
            match prior_shell {
                Some(v) => std::env::set_var("WEFT_APP_SHELL_BIN", v),
                None => std::env::remove_var("WEFT_APP_SHELL_BIN"),
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

        let _env = env_lock().lock().await;
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

        use_test_runtime_dir();

        runtime::supervise(
            session_id,
            launch::LaunchPackage::unresolved("test.abort.startup"),
            grants::SessionGrants::default(),
            Arc::clone(&registry),
            abort_rx,
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
        let _env = env_lock().lock().await;
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

        use_test_runtime_dir();

        runtime::supervise(
            session_id,
            launch::LaunchPackage::unresolved("test.spawn.fail"),
            grants::SessionGrants::default(),
            Arc::clone(&registry),
            abort_rx,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            registry.lock().await.state(session_id),
            AppStateKind::Stopped
        ));

        let broadcast =
            tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(matches!(
            broadcast,
            Ok(Ok(Response::AppState { session_id: sid, state: AppStateKind::Stopped })) if sid == session_id
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
        let _env = env_lock().lock().await;
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
        let _env = env_lock().lock().await;
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
        let _env = env_lock().blocking_lock();
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

    /// Outcome of one supervised session with scripted runtime and app shell.
    #[cfg(unix)]
    struct SessionRun {
        notifications: Vec<Response>,
        state: AppStateKind,
        elapsed: std::time::Duration,
        runtime_pid: Option<i32>,
    }

    /// Runs `supervise` with shell scripts standing in for weft-runtime and
    /// weft-app-shell. The runtime script's PID is recorded so a test can check
    /// that the session did not leave it running.
    #[cfg(unix)]
    async fn run_scripted_session(runtime_body: &str, shell_body: Option<&str>) -> SessionRun {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "weft_session_{}_{}",
            std::process::id(),
            SESSION_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("runtime.pid");
        let write = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let runtime = write(
            "runtime.sh",
            &format!("echo $$ > {}\n{runtime_body}", pid_file.display()),
        );
        let shell = shell_body.map(|body| write("shell.sh", body));

        let _restore = RestoreOnDrop {
            prior: SCRIPTED_ENV
                .iter()
                .map(|n| (*n, std::env::var(n).ok()))
                .collect(),
            dir: dir.clone(),
        };
        // SAFETY: callers hold env_lock and use a current_thread runtime.
        unsafe {
            std::env::set_var("WEFT_RUNTIME_BIN", &runtime);
            match &shell {
                Some(path) => std::env::set_var("WEFT_APP_SHELL_BIN", path),
                None => std::env::remove_var("WEFT_APP_SHELL_BIN"),
            }
            std::env::set_var("WEFT_DISABLE_CGROUP", "1");
        }

        let registry = make_registry();
        let mut rx = registry.lock().await.subscribe();
        let session_id = registry.lock().await.launch("test.app");
        let abort_rx = registry.lock().await.register_abort(session_id);
        let started = std::time::Instant::now();
        use_test_runtime_dir();
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            runtime::supervise(
                session_id,
                launch::LaunchPackage::unresolved("test.app"),
                grants::SessionGrants::default(),
                Arc::clone(&registry),
                abort_rx,
                None,
            ),
        )
        .await
        .expect("session did not settle")
        .unwrap();
        let elapsed = started.elapsed();

        let mut notifications = Vec::new();
        while let Ok(n) = rx.try_recv() {
            notifications.push(n);
        }
        let state = registry.lock().await.state(session_id);
        let runtime_pid = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse().ok());

        SessionRun {
            notifications,
            state,
            elapsed,
            runtime_pid,
        }
    }

    #[cfg(unix)]
    static SESSION_RUN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    #[cfg(unix)]
    const SCRIPTED_ENV: [&str; 3] = [
        "WEFT_RUNTIME_BIN",
        "WEFT_APP_SHELL_BIN",
        "WEFT_DISABLE_CGROUP",
    ];

    /// Restores the scripted-session environment and removes its scripts,
    /// also when an assertion fails.
    #[cfg(unix)]
    struct RestoreOnDrop {
        prior: Vec<(&'static str, Option<String>)>,
        dir: std::path::PathBuf,
    }

    #[cfg(unix)]
    impl Drop for RestoreOnDrop {
        fn drop(&mut self) {
            // SAFETY: the owning test holds env_lock on a current_thread runtime.
            unsafe {
                for (name, value) in &self.prior {
                    match value {
                        Some(v) => std::env::set_var(name, v),
                        None => std::env::remove_var(name),
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(unix)]
    fn became_ready(run: &SessionRun) -> bool {
        run.notifications
            .iter()
            .any(|n| matches!(n, Response::AppReady { .. }))
    }

    #[cfg(unix)]
    fn process_alive(pid: i32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
            && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|stat| stat.split_whitespace().nth(2) == Some("Z"))
                .unwrap_or(true)
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn session_is_not_ready_until_app_shell_is_ready() {
        let _env = env_lock().lock().await;
        let run = run_scripted_session(
            "echo READY $WEFT_READY_TOKEN\nexec sleep 30",
            Some("sleep 0.3\nexit 1"),
        )
        .await;
        assert!(!became_ready(&run), "{:?}", run.notifications);
        assert!(matches!(run.state, AppStateKind::Stopped));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn session_stops_when_runtime_exits_before_app_shell_is_ready() {
        let _env = env_lock().lock().await;
        let run = run_scripted_session(
            "echo READY $WEFT_READY_TOKEN\nexit 0",
            Some("exec sleep 30"),
        )
        .await;
        assert!(!became_ready(&run), "{:?}", run.notifications);
        assert!(matches!(run.state, AppStateKind::Stopped));
        assert!(run.elapsed < std::time::Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn session_without_app_shell_never_becomes_ready() {
        let _env = env_lock().lock().await;
        let run = run_scripted_session("echo READY $WEFT_READY_TOKEN\nexec sleep 30", None).await;
        assert!(!became_ready(&run), "{:?}", run.notifications);
        assert!(matches!(run.state, AppStateKind::Stopped));
        assert!(
            run.runtime_pid.is_none(),
            "runtime started without a UI host"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn readiness_requires_the_session_token() {
        let _env = env_lock().lock().await;
        // Application output can contain READY lines; only the token counts.
        let run = run_scripted_session(
            "echo READY $WEFT_READY_TOKEN\nexec sleep 30",
            Some("echo READY\necho READY 0123456789abcdef\nsleep 0.3"),
        )
        .await;
        assert!(!became_ready(&run), "{:?}", run.notifications);
        assert!(matches!(run.state, AppStateKind::Stopped));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn app_shell_exit_ends_running_session_and_runtime() {
        let _env = env_lock().lock().await;
        let run = run_scripted_session(
            "echo READY $WEFT_READY_TOKEN\nexec sleep 30",
            Some("echo READY $WEFT_READY_TOKEN\nexec sleep 0.3"),
        )
        .await;
        assert!(became_ready(&run), "{:?}", run.notifications);
        assert!(matches!(run.state, AppStateKind::Stopped));
        assert!(run.elapsed < std::time::Duration::from_secs(10));
        let pid = run.runtime_pid.expect("runtime started");
        assert!(!process_alive(pid), "runtime {pid} left running");
    }
}
