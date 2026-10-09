use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use weft_ipc_types::AppdToCompositor;

use crate::Registry;
use crate::compositor_client::CompositorSender;
use crate::ipc::{AppStateKind, Response};

#[cfg(unix)]
pub(crate) async fn spawn_ipc_relay(
    session_id: u64,
    socket_path: PathBuf,
    broadcast: tokio::sync::broadcast::Sender<Response>,
) -> Option<tokio::sync::mpsc::Sender<String>> {
    let _ = std::fs::remove_file(&socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path).ok()?;
    let (html_to_wasm_tx, mut html_to_wasm_rx) = tokio::sync::mpsc::channel::<String>(64);
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            tracing::warn!(session_id, "IPC relay: failed to accept connection");
            let _ = std::fs::remove_file(&socket_path);
            return;
        };
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
        let _ = std::fs::remove_file(&socket_path);
    });
    Some(html_to_wasm_tx)
}

#[cfg(not(unix))]
pub(crate) async fn spawn_ipc_relay(
    _session_id: u64,
    _socket_path: PathBuf,
    _broadcast: tokio::sync::broadcast::Sender<Response>,
) -> Option<tokio::sync::mpsc::Sender<String>> {
    None
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

/// Longest line read from a child's output; longer lines are split.
const MAX_LINE: u64 = 64 * 1024;

fn readiness_token() -> std::io::Result<String> {
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

fn resolve_preopens(app_id: &str) -> Vec<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Pkg {
        capabilities: Option<Vec<String>>,
    }
    #[derive(serde::Deserialize)]
    struct M {
        package: Pkg,
    }

    let pkg_dir = crate::app_store_roots().into_iter().find_map(|root| {
        let dir = root.join(app_id);
        if dir.join("wapp.toml").exists() {
            Some(dir)
        } else {
            None
        }
    });

    let caps = match pkg_dir {
        None => return Vec::new(),
        Some(dir) => {
            let Ok(text) = std::fs::read_to_string(dir.join("wapp.toml")) else {
                return Vec::new();
            };
            match toml::from_str::<M>(&text) {
                Ok(m) => m.package.capabilities.unwrap_or_default(),
                Err(_) => return Vec::new(),
            }
        }
    };

    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return Vec::new(),
    };

    let mut preopens = Vec::new();
    for cap in &caps {
        match cap.as_str() {
            "fs:rw:app-data" | "fs:read:app-data" => {
                let data_dir = home
                    .join(".local/share/weft/apps")
                    .join(app_id)
                    .join("data");
                let _ = std::fs::create_dir_all(&data_dir);
                preopens.push((data_dir.to_string_lossy().into_owned(), "/data".to_string()));
            }
            "fs:rw:xdg-documents" | "fs:read:xdg-documents" => {
                let docs = home.join("Documents");
                if docs.exists() {
                    preopens.push((
                        docs.to_string_lossy().into_owned(),
                        "/xdg/documents".to_string(),
                    ));
                }
            }
            other => {
                tracing::debug!(capability = other, "not mapped to preopen; skipped");
            }
        }
    }
    preopens
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
) -> std::io::Result<tokio::process::Child> {
    let child = tokio::process::Command::new(bin)
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
    allowed_paths: &[(String, String)],
) -> Option<(PathBuf, tokio::process::Child)> {
    let bin = std::env::var("WEFT_FILE_PORTAL_BIN").ok()?;
    let socket = portal_socket_path(session_id)?;
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg(&socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (host, _) in allowed_paths {
        cmd.arg("--allow").arg(host);
    }
    let child = cmd.kill_on_drop(true).spawn().ok()?;
    tracing::info!(session_id, socket = %socket.display(), "file portal spawned");
    Some((socket, child))
}

pub(crate) async fn supervise(
    session_id: u64,
    app_id: &str,
    registry: Registry,
    abort_rx: tokio::sync::oneshot::Receiver<()>,
    compositor_tx: Option<CompositorSender>,
    ipc_socket_path: Option<std::path::PathBuf>,
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
    let token = match readiness_token() {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(session_id, %app_id, error = %e, "cannot create readiness token");
            return stop_unstarted(&registry, session_id).await;
        }
    };

    let (mount_orch, store_override) =
        crate::mount::MountOrchestrator::mount_if_needed(app_id, session_id);

    let preopens = resolve_preopens(app_id);
    let portal = spawn_file_portal(session_id, &preopens);

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

    if let Some(ref sock) = ipc_socket_path {
        cmd.arg("--ipc-socket").arg(sock);
    }

    if let Some(ref root) = store_override {
        cmd.env("WEFT_APP_STORE", root);
    }

    if let Some((ref sock, _)) = portal {
        cmd.env("WEFT_FILE_PORTAL_SOCKET", sock);
    }

    for (host, guest) in &preopens {
        cmd.arg("--preopen").arg(format!("{host}::{guest}"));
    }

    let mut session = OwnedSession {
        session_id,
        runtime: None,
        app_shell: None,
        portal,
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

    let mut app_shell = match spawn_app_shell(&shell_bin, session_id, app_id, &token) {
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

/// The processes, portal, compositor association and image mount a session
/// owns. `settle` releases all of them on every exit path: spawn failure,
/// readiness failure, timeout, abort and child exit. The IPC relay is not yet
/// owned here.
struct OwnedSession {
    session_id: u64,
    runtime: Option<tokio::process::Child>,
    app_shell: Option<tokio::process::Child>,
    portal: Option<(PathBuf, tokio::process::Child)>,
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
        let mut reg = registry.lock().await;
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
