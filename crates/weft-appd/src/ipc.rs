use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Request {
    LaunchApp { app_id: String, surface_id: u64 },
    TerminateApp { session_id: u64 },
    QueryRunning,
    QueryAppState { session_id: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Response {
    LaunchAck {
        session_id: u64,
    },
    AppReady {
        session_id: u64,
    },
    RunningApps {
        session_ids: Vec<u64>,
    },
    AppState {
        session_id: u64,
        state: AppStateKind,
    },
    Error {
        code: u32,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStateKind {
    Starting,
    Running,
    Stopping,
    Stopped,
    NotFound,
}

pub async fn read_frame(
    reader: &mut (impl AsyncReadExt + Unpin),
) -> anyhow::Result<Option<Request>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    let req = rmp_serde::from_slice(&body)?;
    Ok(Some(req))
}

pub async fn write_frame(
    writer: &mut (impl AsyncWriteExt + Unpin),
    response: &Response,
) -> anyhow::Result<()> {
    let body = rmp_serde::to_vec(response)?;
    let len = (body.len() as u32).to_le_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&body).await?;
    Ok(())
}
