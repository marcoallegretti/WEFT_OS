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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_msgpack_roundtrip() {
        let req = Request::LaunchApp {
            app_id: "com.example.app".into(),
            surface_id: 42,
        };
        let bytes = rmp_serde::to_vec(&req).unwrap();
        let decoded: Request = rmp_serde::from_slice(&bytes).unwrap();
        match decoded {
            Request::LaunchApp { app_id, surface_id } => {
                assert_eq!(app_id, "com.example.app");
                assert_eq!(surface_id, 42);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_msgpack_roundtrip() {
        let resp = Response::LaunchAck { session_id: 7 };
        let bytes = rmp_serde::to_vec(&resp).unwrap();
        let decoded: Response = rmp_serde::from_slice(&bytes).unwrap();
        match decoded {
            Response::LaunchAck { session_id } => assert_eq!(session_id, 7),
            _ => panic!("wrong variant"),
        }
    }

    #[tokio::test]
    async fn frame_write_read_roundtrip() {
        let resp = Response::RunningApps {
            session_ids: vec![1, 2, 3],
        };
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &resp).await.unwrap();

        assert_eq!(
            buf.len() as u32,
            u32::from_le_bytes(buf[..4].try_into().unwrap()) + 4
        );

        let req_to_write = Request::QueryRunning;
        let mut req_buf: Vec<u8> = Vec::new();
        let body = rmp_serde::to_vec(&req_to_write).unwrap();
        let len = (body.len() as u32).to_le_bytes();
        req_buf.extend_from_slice(&len);
        req_buf.extend_from_slice(&body);

        let mut cursor = std::io::Cursor::new(req_buf);
        let decoded = read_frame(&mut cursor).await.unwrap();
        assert!(matches!(decoded, Some(Request::QueryRunning)));
    }

    #[tokio::test]
    async fn read_frame_eof_returns_none() {
        let mut empty = std::io::Cursor::new(Vec::<u8>::new());
        let result = read_frame(&mut empty).await.unwrap();
        assert!(result.is_none());
    }
}
