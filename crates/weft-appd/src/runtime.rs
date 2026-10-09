use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use weft_ipc_types::AppdToCompositor;

use crate::Registry;
use crate::compositor_client::CompositorSender;
use crate::ipc::{AppStateKind, Response};

/// The per-session relay between the component's IPC socket and the app
/// bridge. It belongs to the session: closing it stops the relay task and
/// removes the socket, whether or not the component ever connected.
pub(crate) struct IpcRelay {
    task: tokio::task::JoinHandle<()>,
    socket_path: PathBuf,
}

impl IpcRelay {
    fn close(self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Listens on `socket_path` for the session's component and relays
/// newline-delimited messages between it and the returned sender (towards
/// the component) and the broadcast channel (from the component).
#[cfg(unix)]
pub(crate) fn spawn_ipc_relay(
    session_id: u64,
    socket_path: PathBuf,
    broadcast: tokio::sync::broadcast::Sender<Response>,
) -> std::io::Result<(tokio::sync::mpsc::Sender<String>, IpcRelay)> {
    let _ = std::fs::remove_file(&socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let (html_to_wasm_tx, mut html_to_wasm_rx) = tokio::sync::mpsc::channel::<String>(64);
    let task = tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            tracing::warn!(session_id, "IPC relay: failed to accept connection");
            return;
        };
        // One component per session: no further connections are accepted.
        drop(listener);
        tracing::debug!(session_id, "IPC relay: component connected");
        let (reader, writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let mut writer = BufWriter::new(writer);
        loop {
            let mut line = String::new();
            tokio::select! {
                n = reader.read_line(&mut line) => {
                    match n {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let payload = line.trim_end().to_owned();
                            let _ = broadcast.send(Response::IpcMessage { session_id, payload });
                        }
                    }
                }
                msg = html_to_wasm_rx.recv() => {
                    match msg {
                        Some(payload) => {
                            let mut data = payload;
                            data.push('\n');
                            if writer.write_all(data.as_bytes()).await.is_err()
                                || writer.flush().await.is_err()
                            {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });
    Ok((html_to_wasm_tx, IpcRelay { task, socket_path }))
}

#[cfg(not(unix))]
pub(crate) fn spawn_ipc_relay(
    _session_id: u64,
    _socket_path: PathBuf,
    _broadcast: tokio::sync::broadcast::Sender<Response>,
) -> std::io::Result<(tokio::sync::mpsc::Sender<String>, IpcRelay)> {
    Err(std::io::ErrorKind::Unsupported.into())
}

const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Environment variable carrying a session's readiness token to its children.
///
/// A child reports readiness by printing `READY <token>` on its own stdout.
/// Application code shares that stdout (Wasm guests through inherited stdio,
/// pages through `window.alert`) but cannot read the token, because the
/// runtime passes the guest only the variables it names and pages cannot read
/// the host process environment.
const READY_TOKEN_ENV: &str = "WEFT_READY_TOKEN";

/// Environment variable carrying the session's application bridge credential
/// to weft-app-shell.
const BRIDGE_TOKEN_ENV: &str = "WEFT_BRIDGE_TOKEN";

/// Longest line read from a child's output; longer lines are split.
const MAX_LINE: u64 = 64 * 1024;

/// A random 128-bit credential as 32 hex characters.
pub(crate) fn random_token() -> std::io::Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Reads one line of at most MAX_LINE bytes, decoded lossily. Returns None at
/// end of output.
async fn read_child_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    use tokio::io::AsyncReadExt;
    let mut line = Vec::new();
    let n = (&mut *reader)
        .take(MAX_LINE)
        .read_until(b'\n', &mut line)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&line).trim_end().to_owned()))
}

/// `systemd-run` options that place the runtime in a transient user scope.
///
/// A scope runs the command in the foreground as systemd-run's own process,
/// so its stdout and exit status reach appd directly. `--wait` only applies
/// to service units; systemd rejects it together with `--scope`.
const SYSTEMD_SCOPE_ARGS: &[&str] = &[
    "--user",
    "--scope",
    "--collect",
    "--slice=weft-apps.slice",
    "-p",
    "CPUQuota=200%",
    "-p",
    "MemoryMax=512M",
];

fn systemd_cgroup_available() -> bool {
    if std::env::var("WEFT_DISABLE_CGROUP").is_ok() {
        return false;
    }
    let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") else {
        return false;
    };
    std::path::Path::new(&runtime_dir)
        .join("systemd/private")
        .exists()
}

async fn kill_portal(portal: Option<(PathBuf, tokio::process::Child)>) {
    if let Some((sock, mut child)) = portal {
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = std::fs::remove_file(&sock);
    }
}

fn spawn_app_shell(
    bin: &str,
    session_id: u64,
    app_id: &str,
    token: &str,
    bridge_token: Option<String>,
) -> std::io::Result<tokio::process::Child> {
    let mut command = tokio::process::Command::new(bin);
    if let Some(bridge_token) = bridge_token {
        // The page holds this credential; it must differ from the readiness
        // token, which page output could otherwise use to fake readiness.
        command.env(BRIDGE_TOKEN_ENV, bridge_token);
    }
    let child = command
        .arg(app_id)
        .arg(session_id.to_string())
        .env(READY_TOKEN_ENV, token)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    tracing::info!(session_id, %app_id, bin, "app shell spawned");
    Ok(child)
}

fn portal_socket_path(session_id: u64) -> Option<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let dir = PathBuf::from(runtime_dir).join("weft");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("portal-{session_id}.sock")))
}

fn spawn_file_portal(
    session_id: u64,
    dirs: &[crate::grants::GrantedDir],
) -> Option<(PathBuf, tokio::process::Child)> {
    let bin = std::env::var("WEFT_FILE_PORTAL_BIN").ok()?;
    let socket = portal_socket_path(session_id)?;
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg(&socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for dir in dirs {
        let flag = match dir.access {
            weft_ipc_types::capability::Access::Read => "--allow-read",
            weft_ipc_types::capability::Access::ReadWrite => "--allow",
        };
        cmd.arg(flag).arg(&dir.host);
    }
    let child = cmd.kill_on_drop(true).spawn().ok()?;
    tracing::info!(session_id, socket = %socket.display(), "file portal spawned");
    Some((socket, child))
}

pub(crate) async fn supervise(
    session_id: u64,
    app_id: &str,
    grants: crate::grants::SessionGrants,
    registry: Registry,
    abort_rx: tokio::sync::oneshot::Receiver<()>,
    compositor_tx: Option<CompositorSender>,
) -> anyhow::Result<()> {
    let mut abort_rx = abort_rx;
    let bin = match std::env::var("WEFT_RUNTIME_BIN") {
        Ok(b) => b,
        Err(_) => {
            tracing::debug!(session_id, %app_id, "WEFT_RUNTIME_BIN not set; skipping process spawn");
            return stop_unstarted(&registry, session_id).await;
        }
    };

    let Ok(shell_bin) = std::env::var("WEFT_APP_SHELL_BIN") else {
        tracing::warn!(session_id, %app_id, "WEFT_APP_SHELL_BIN not set; no UI host to start");
        return stop_unstarted(&registry, session_id).await;
    };
    let token = match random_token() {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(session_id, %app_id, error = %e, "cannot create readiness token");
            return stop_unstarted(&registry, session_id).await;
        }
    };

    let Some(ipc_socket_path) = crate::session_ipc_socket_path(session_id) else {
        tracing::warn!(session_id, %app_id, "no runtime directory for the IPC socket");
        return stop_unstarted(&registry, session_id).await;
    };
    let broadcast = registry.lock().await.broadcast().clone();
    let relay = match spawn_ipc_relay(session_id, ipc_socket_path.clone(), broadcast) {
        Ok((tx, relay)) => {
            registry.lock().await.register_ipc_sender(session_id, tx);
            relay
        }
        Err(e) => {
            tracing::warn!(session_id, %app_id, error = %e, "cannot open the IPC socket");
            return stop_unstarted(&registry, session_id).await;
        }
    };

    let (mount_orch, store_override) =
        crate::mount::MountOrchestrator::mount_if_needed(app_id, session_id);

    let portal = spawn_file_portal(session_id, &grants.dirs);

    let mut cmd = if systemd_cgroup_available() {
        let mut c = tokio::process::Command::new("systemd-run");
        c.args(SYSTEMD_SCOPE_ARGS).arg("--").arg(&bin);
        c
    } else {
        tokio::process::Command::new(&bin)
    };
    cmd.arg(app_id)
        .arg(session_id.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .env(READY_TOKEN_ENV, &token)
        .kill_on_drop(true);

    cmd.arg("--ipc-socket").arg(&ipc_socket_path);

    if let Some(ref root) = store_override {
        cmd.env("WEFT_APP_STORE", root);
    }

    if let Some((ref sock, _)) = portal {
        cmd.env("WEFT_FILE_PORTAL_SOCKET", sock);
    }

    for dir in &grants.dirs {
        cmd.arg("--preopen").arg(dir.preopen_arg());
    }
    for capability in &grants.imports {
        cmd.arg("--grant").arg(capability.to_string());
    }

    let mut session = OwnedSession {
        session_id,
        runtime: None,
        app_shell: None,
        portal,
        relay: Some(relay),
        mount: mount_orch,
        compositor_tx,
        surface_announced: false,
    };

    let mut runtime = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return session
                .settle(&registry, &format!("failed to spawn runtime: {e}"))
                .await;
        }
    };
    if let Some(tx) = &session.compositor_tx {
        let pid = runtime.id().unwrap_or(0);
        // A full queue (compositor disconnected) must not stall the session.
        session.surface_announced = tx
            .try_send(AppdToCompositor::AppSurfaceCreated {
                app_id: app_id.to_owned(),
                session_id,
                pid,
            })
            .is_ok();
        if !session.surface_announced {
            tracing::warn!(
                session_id,
                "compositor queue unavailable; surface not announced"
            );
        }
    }
    let runtime_stdout = runtime.stdout.take().expect("stdout piped");
    tokio::spawn(drain_stderr(
        runtime.stderr.take().expect("stderr piped"),
        session_id,
        "runtime",
    ));
    session.runtime = Some(runtime);

    // The component must initialize before the UI host starts.
    let runtime_stdout = match wait_for_child_ready(runtime_stdout, &token, &mut abort_rx).await {
        Ok(reader) => reader,
        Err(reason) => {
            return session
                .settle(&registry, &format!("runtime {reason}"))
                .await;
        }
    };
    tokio::spawn(drain_stdout(runtime_stdout, session_id));

    let bridge_token = registry.lock().await.bridge_token(session_id);
    let mut app_shell = match spawn_app_shell(&shell_bin, session_id, app_id, &token, bridge_token)
    {
        Ok(child) => child,
        Err(e) => {
            return session
                .settle(&registry, &format!("failed to spawn app shell: {e}"))
                .await;
        }
    };
    let shell_stdout = app_shell.stdout.take().expect("stdout piped");
    tokio::spawn(drain_stderr(
        app_shell.stderr.take().expect("stderr piped"),
        session_id,
        "app shell",
    ));
    session.app_shell = Some(app_shell);

    // Running requires the UI host to have presented the loaded document.
    let shell_stdout = tokio::select! {
        r = wait_for_child_ready(shell_stdout, &token, &mut abort_rx) => {
            r.map_err(|reason| format!("app shell {reason}"))
        }
        status = session.runtime.as_mut().expect("runtime spawned").wait() => {
            Err(format!("runtime exited before the app shell was ready ({status:?})"))
        }
    };
    let shell_stdout = match shell_stdout {
        Ok(reader) => reader,
        Err(reason) => return session.settle(&registry, &reason).await,
    };
    tokio::spawn(drain_stdout(shell_stdout, session_id));

    {
        let mut reg = registry.lock().await;
        reg.set_state(session_id, AppStateKind::Running);
        let _ = reg.broadcast().send(Response::AppReady {
            session_id,
            app_id: app_id.to_owned(),
        });
    }
    tracing::info!(session_id, %app_id, "app ready");

    let reason = tokio::select! {
        status = session.runtime.as_mut().expect("runtime spawned").wait() => {
            format!("runtime exited ({status:?})")
        }
        status = session.app_shell.as_mut().expect("app shell spawned").wait() => {
            format!("app shell exited ({status:?})")
        }
        _ = &mut abort_rx => "terminate requested".to_owned(),
    };
    session.settle(&registry, &reason).await
}

/// Marks a session that never started any process as stopped.
async fn stop_unstarted(registry: &Registry, session_id: u64) -> anyhow::Result<()> {
    let mut reg = registry.lock().await;
    reg.set_state(session_id, AppStateKind::Stopped);
    reg.remove_abort_sender(session_id);
    let _ = reg.broadcast().send(Response::AppState {
        session_id,
        state: AppStateKind::Stopped,
    });
    Ok(())
}

/// The processes, IPC relay, portal, compositor association and image mount
/// a session owns. `settle` releases all of them on every exit path: spawn
/// failure, readiness failure, timeout, abort and child exit.
struct OwnedSession {
    session_id: u64,
    runtime: Option<tokio::process::Child>,
    app_shell: Option<tokio::process::Child>,
    portal: Option<(PathBuf, tokio::process::Child)>,
    relay: Option<IpcRelay>,
    mount: crate::mount::MountOrchestrator,
    compositor_tx: Option<CompositorSender>,
    surface_announced: bool,
}

impl OwnedSession {
    async fn settle(self, registry: &Registry, reason: &str) -> anyhow::Result<()> {
        let session_id = self.session_id;
        tracing::info!(session_id, reason, "stopping session");
        kill_child(self.app_shell).await;
        kill_child(self.runtime).await;
        if self.surface_announced
            && let Some(tx) = &self.compositor_tx
            && tx
                .try_send(AppdToCompositor::AppSurfaceDestroyed { session_id })
                .is_err()
        {
            tracing::warn!(
                session_id,
                "compositor queue unavailable; surface not released"
            );
        }
        self.mount.umount();
        kill_portal(self.portal).await;
        if let Some(relay) = self.relay {
            relay.close();
        }
        let mut reg = registry.lock().await;
        reg.remove_ipc_sender(session_id);
        reg.set_state(session_id, AppStateKind::Stopped);
        reg.remove_abort_sender(session_id);
        let _ = reg.broadcast().send(Response::AppState {
            session_id,
            state: AppStateKind::Stopped,
        });
        Ok(())
    }
}

async fn kill_child(child: Option<tokio::process::Child>) {
    if let Some(mut child) = child {
        let _ = child.kill().await;
    }
}

/// Waits for a child to print `READY <token>`, for at most READY_TIMEOUT.
/// Returns why it did not become ready otherwise.
async fn wait_for_child_ready(
    stdout: tokio::process::ChildStdout,
    token: &str,
    abort_rx: &mut tokio::sync::oneshot::Receiver<()>,
) -> Result<BufReader<tokio::process::ChildStdout>, String> {
    tokio::select! {
        r = tokio::time::timeout(READY_TIMEOUT, wait_for_ready(stdout, token)) => match r {
            Ok(Ok(reader)) => Ok(reader),
            Ok(Err(e)) => Err(format!("did not become ready: {e}")),
            Err(_) => Err(format!("not ready after {}s", READY_TIMEOUT.as_secs())),
        },
        _ = abort_rx => Err("startup aborted".to_owned()),
    }
}

async fn wait_for_ready(
    stdout: tokio::process::ChildStdout,
    token: &str,
) -> anyhow::Result<BufReader<tokio::process::ChildStdout>> {
    let expected = format!("READY {token}");
    let mut reader = BufReader::new(stdout);
    loop {
        match read_child_line(&mut reader).await? {
            None => return Err(anyhow::anyhow!("stdout closed without READY signal")),
            // Earlier output without a trailing newline can precede the
            // readiness line; the secret token alone establishes it.
            Some(line) if line.ends_with(&expected) => return Ok(reader),
            Some(_) => {}
        }
    }
}

async fn drain_stdout(mut reader: BufReader<tokio::process::ChildStdout>, session_id: u64) {
    while let Ok(Some(line)) = read_child_line(&mut reader).await {
        tracing::debug!(session_id, stdout = %line, "child stdout");
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr, session_id: u64, process: &str) {
    let mut reader = BufReader::new(stderr);
    while let Ok(Some(line)) = read_child_line(&mut reader).await {
        tracing::info!(session_id, process, stderr = %line, "child stderr");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::SYSTEMD_SCOPE_ARGS;

    /// systemd-run checks its options and their combinations before it
    /// connects to the service manager. Run against an empty runtime
    /// directory and bus address, it must get as far as the connection
    /// attempt; any option error ends earlier with a different message. This
    /// does not validate the `-p` property values, which only the manager
    /// checks, and never starts a real scope.
    #[test]
    fn systemd_run_accepts_scope_options() {
        let empty = std::env::temp_dir().join(format!("weft-scope-test-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let output = std::process::Command::new("systemd-run")
            .args(SYSTEMD_SCOPE_ARGS)
            .args(["--", "true"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("XDG_RUNTIME_DIR", &empty)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/bus", empty.display()),
            )
            .output();
        std::fs::remove_dir_all(&empty).unwrap();
        let output = output.expect("systemd-run is required to check the scope options");
        let stderr = String::from_utf8_lossy(&output.stderr);
        // systemd 255 says "Failed to connect to bus"; later releases name the
        // transport, e.g. "Failed to connect to user scope bus via local transport".
        assert!(
            stderr.starts_with("Failed to connect to"),
            "systemd-run did not reach the manager connection: {stderr}"
        );
    }
}
