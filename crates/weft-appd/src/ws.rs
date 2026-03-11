use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::{Registry, dispatch, ipc::Request, ipc::Response};

pub async fn handle_ws_connection(
    stream: tokio::net::TcpStream,
    registry: Registry,
    broadcast_rx: broadcast::Receiver<Response>,
) -> anyhow::Result<()> {
    let ws_stream = accept_async(stream).await?;
    let (mut ws_write, mut ws_read) = ws_stream.split();
    let mut broadcast_rx = broadcast_rx;

    loop {
        tokio::select! {
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let req: Request = match serde_json::from_str(&text) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(error = %e, "invalid WS request");
                                continue;
                            }
                        };
                        tracing::debug!(?req, "ws request");
                        let resp = dispatch(req, &registry).await;
                        let json = serde_json::to_string(&resp)?;
                        ws_write.send(Message::Text(json)).await?;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "ws error");
                        break;
                    }
                }
            }
            notification = broadcast_rx.recv() => {
                match notification {
                    Ok(resp) => {
                        let json = serde_json::to_string(&resp)?;
                        ws_write.send(Message::Text(json)).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    Ok(())
}
