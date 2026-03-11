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
        Ok(Ok(())) => {
            registry
                .lock()
                .await
                .set_state(session_id, AppStateKind::Running);
            let _ = registry
                .lock()
                .await
                .broadcast()
                .send(Response::AppReady { session_id });
            tracing::info!(session_id, %app_id, "app ready");
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

    let status = child.wait().await?;
    tracing::info!(session_id, %app_id, exit_status = ?status, "process exited");
    registry
        .lock()
        .await
        .set_state(session_id, AppStateKind::Stopped);

    Ok(())
}

async fn wait_for_ready(stdout: tokio::process::ChildStdout) -> anyhow::Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim() == "READY" {
            return Ok(());
        }
    }
    Err(anyhow::anyhow!("stdout closed without READY signal"))
}

async fn drain_stderr(stderr: tokio::process::ChildStderr, session_id: u64) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::warn!(session_id, stderr = %line, "app stderr");
    }
}
