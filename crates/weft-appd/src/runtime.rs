use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::Registry;
use crate::ipc::{AppStateKind, Response};

const READY_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn supervise(
    session_id: u64,
    app_id: &str,
    registry: Registry,
    abort_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let bin = match std::env::var("WEFT_RUNTIME_BIN") {
        Ok(b) => b,
        Err(_) => {
            tracing::debug!(session_id, %app_id, "WEFT_RUNTIME_BIN not set; skipping process spawn");
            return Ok(());
        }
    };

    let mut child = tokio::process::Command::new(&bin)
        .arg(app_id)
        .arg(session_id.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {bin}"))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let ready_result = tokio::time::timeout(READY_TIMEOUT, wait_for_ready(stdout)).await;

    match ready_result {
        Ok(Ok(remaining_stdout)) => {
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
        }
        Ok(Err(e)) => {
            tracing::warn!(session_id, %app_id, error = %e, "stdout read error before READY");
        }
        Err(_elapsed) => {
            tracing::warn!(session_id, %app_id, "READY timeout after 30s; killing process");
            let _ = child.kill().await;
            registry
                .lock()
                .await
                .set_state(session_id, AppStateKind::Stopped);
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

    registry
        .lock()
        .await
        .set_state(session_id, AppStateKind::Stopped);

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
