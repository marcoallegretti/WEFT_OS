use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use weft_ipc_types::AppdToCompositor;

use crate::Registry;
use crate::compositor_client::CompositorSender;
use crate::ipc::{AppStateKind, Response};

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

const READY_TIMEOUT: Duration = Duration::from_secs(30);

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

async fn spawn_app_shell(session_id: u64, app_id: &str) -> Option<tokio::process::Child> {
    let bin = std::env::var("WEFT_APP_SHELL_BIN").ok()?;
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg(app_id)
        .arg(session_id.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            tracing::info!(session_id, %app_id, bin = %bin, "app shell spawned");
            Some(child)
        }
        Err(e) => {
            tracing::warn!(session_id, %app_id, error = %e, "failed to spawn app shell");
            None
        }
    }
}

async fn kill_app_shell(child: Option<tokio::process::Child>) {
    if let Some(mut c) = child {
        let _ = c.kill().await;
        let _ = c.wait().await;
    }
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
    let child = cmd.spawn().ok()?;
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
            let mut reg = registry.lock().await;
            reg.set_state(session_id, AppStateKind::Stopped);
            reg.remove_abort_sender(session_id);
            let _ = reg.broadcast().send(Response::AppState {
                session_id,
                state: AppStateKind::Stopped,
            });
            return Ok(());
        }
    };

    let (mount_orch, store_override) =
        crate::mount::MountOrchestrator::mount_if_needed(app_id, session_id);

    let preopens = resolve_preopens(app_id);
    let portal = spawn_file_portal(session_id, &preopens);

    let mut cmd = if systemd_cgroup_available() {
        let mut c = tokio::process::Command::new("systemd-run");
        c.args([
            "--user",
            "--scope",
            "--wait",
            "--collect",
            "--slice=weft-apps.slice",
            "-p",
            "CPUQuota=200%",
            "-p",
            "MemoryMax=512M",
            "--",
            &bin,
        ]);
        c
    } else {
        tokio::process::Command::new(&bin)
    };
    cmd.arg(app_id)
        .arg(session_id.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

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

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(session_id, %app_id, error = %e, "failed to spawn runtime; marking session stopped");
            kill_portal(portal).await;
            let mut reg = registry.lock().await;
            reg.set_state(session_id, AppStateKind::Stopped);
            let _ = reg.broadcast().send(Response::AppState {
                session_id,
                state: AppStateKind::Stopped,
            });
            return Ok(());
        }
    };

    if let Some(tx) = &compositor_tx {
        let pid = child.id().unwrap_or(0);
        let _ = tx
            .send(AppdToCompositor::AppSurfaceCreated {
                app_id: app_id.to_owned(),
                session_id,
                pid,
            })
            .await;
    }

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let ready_result = tokio::select! {
        r = tokio::time::timeout(READY_TIMEOUT, wait_for_ready(stdout)) => Some(r),
        _ = &mut abort_rx => None,
    };

    let mut app_shell: Option<tokio::process::Child> = None;
    match ready_result {
        Some(Ok(Ok(remaining_stdout))) => {
            registry
                .lock()
                .await
                .set_state(session_id, AppStateKind::Running);
            let _ = registry.lock().await.broadcast().send(Response::AppReady {
                session_id,
                app_id: app_id.to_owned(),
            });
            tracing::info!(session_id, %app_id, "app ready");
            tokio::spawn(drain_stdout(remaining_stdout, session_id));
            app_shell = spawn_app_shell(session_id, app_id).await;
        }
        Some(Ok(Err(e))) => {
            tracing::warn!(session_id, %app_id, error = %e, "stdout read error before READY");
        }
        Some(Err(_elapsed)) => {
            tracing::warn!(session_id, %app_id, "READY timeout after 30s; killing process");
            let _ = child.kill().await;
            kill_portal(portal).await;
            let mut reg = registry.lock().await;
            reg.set_state(session_id, AppStateKind::Stopped);
            reg.remove_abort_sender(session_id);
            let _ = reg.broadcast().send(Response::AppState {
                session_id,
                state: AppStateKind::Stopped,
            });
            return Ok(());
        }
        None => {
            tracing::info!(session_id, %app_id, "abort during startup; killing process");
            let _ = child.kill().await;
            kill_portal(portal).await;
            let mut reg = registry.lock().await;
            reg.set_state(session_id, AppStateKind::Stopped);
            let _ = reg.broadcast().send(Response::AppState {
                session_id,
                state: AppStateKind::Stopped,
            });
            return Ok(());
        }
    }

    tokio::spawn(drain_stderr(stderr, session_id));

    tokio::select! {
        status = child.wait() => {
            tracing::info!(session_id, %app_id, exit_status = ?status, "process exited");
        }
        _ = abort_rx => {
            tracing::info!(session_id, %app_id, "abort received; sending SIGTERM");
            let _ = child.kill().await;
        }
    }

    kill_app_shell(app_shell).await;

    if let Some(tx) = &compositor_tx {
        let _ = tx
            .send(AppdToCompositor::AppSurfaceDestroyed { session_id })
            .await;
    }

    mount_orch.umount();

    kill_portal(portal).await;

    {
        let mut reg = registry.lock().await;
        reg.set_state(session_id, AppStateKind::Stopped);
        reg.remove_abort_sender(session_id);
        let _ = reg.broadcast().send(Response::AppState {
            session_id,
            state: AppStateKind::Stopped,
        });
    }

    Ok(())
}

async fn wait_for_ready(
    stdout: tokio::process::ChildStdout,
) -> anyhow::Result<BufReader<tokio::process::ChildStdout>> {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow::anyhow!("stdout closed without READY signal"));
        }
        if line.trim() == "READY" {
            return Ok(reader);
        }
    }
}

async fn drain_stdout(mut reader: BufReader<tokio::process::ChildStdout>, session_id: u64) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => tracing::debug!(session_id, stdout = %line.trim_end(), "app stdout"),
        }
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr, session_id: u64) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::warn!(session_id, stderr = %line, "app stderr");
    }
}
