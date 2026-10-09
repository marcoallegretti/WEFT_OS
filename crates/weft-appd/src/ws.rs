//! Browser-facing WebSocket endpoint with role-separated authority.
//!
//! Every connection starts with a `HELLO` message naming its role and
//! credential:
//!
//! - `{"type":"HELLO","role":"system","token":…}` — the trusted system shell,
//!   holding the token appd writes to `$XDG_RUNTIME_DIR/weft/appd.systoken`.
//!   It may use the catalog, lifecycle and gesture requests and receives
//!   session state notifications, but never application payloads.
//! - `{"type":"HELLO","role":"app","session_id":N,"token":…}` — the bridge of
//!   one application session, holding the token appd gave that session's app
//!   shell. It may only exchange `{"type":"APP_MESSAGE","payload":"…"}`
//!   messages with its own session's component.
//!
//! Connections that do not complete the upgrade and authenticate within
//! `HELLO_TIMEOUT` are closed.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::ipc::{AppStateKind, Request, Response};
use crate::{Registry, dispatch};

const HELLO_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(5)
};
/// Largest WebSocket message accepted from any client.
const MAX_MESSAGE: usize = 1024 * 1024;
/// Largest application payload forwarded to a component.
const MAX_APP_PAYLOAD: usize = 64 * 1024;

pub struct WsAuth {
    pub system_token: String,
}

/// Compares credentials without exiting early on the first differing byte.
pub(crate) fn tokens_match(expected: &str, given: &str) -> bool {
    let (expected, given) = (expected.as_bytes(), given.as_bytes());
    expected.len() == given.len()
        && expected
            .iter()
            .zip(given)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[derive(Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum Hello {
    System { token: String },
    App { session_id: u64, token: String },
}

#[derive(Deserialize)]
struct HelloEnvelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(flatten)]
    hello: Hello,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum AppRequest {
    AppMessage { payload: String },
}

enum Role {
    System,
    App(u64),
}

pub async fn handle_ws_connection(
    stream: tokio::net::TcpStream,
    registry: Registry,
    broadcast_rx: broadcast::Receiver<Response>,
    auth: Arc<WsAuth>,
) -> anyhow::Result<()> {
    let config = WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE),
        max_frame_size: Some(MAX_MESSAGE),
        ..Default::default()
    };
    // The upgrade and HELLO share one deadline, so a client that never
    // completes either holds nothing for longer than HELLO_TIMEOUT.
    let handshake = async {
        let ws_stream = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await?;
        let (ws_write, mut ws_read) = ws_stream.split();
        let hello = match ws_read.next().await {
            Some(Ok(Message::Text(text))) => serde_json::from_str::<HelloEnvelope>(&text).ok(),
            _ => None,
        };
        anyhow::Ok((ws_write, ws_read, hello))
    };
    let Ok(handshake) = tokio::time::timeout(HELLO_TIMEOUT, handshake).await else {
        tracing::debug!("WebSocket client did not complete its handshake in time");
        return Ok(());
    };
    let (mut ws_write, mut ws_read, hello) = handshake?;
    let role = match hello {
        Some(HelloEnvelope {
            kind,
            hello: Hello::System { token },
        }) if kind == "HELLO" && tokens_match(&auth.system_token, &token) => Role::System,
        Some(HelloEnvelope {
            kind,
            hello: Hello::App { session_id, token },
        }) if kind == "HELLO" && registry.lock().await.authorize_bridge(session_id, &token) => {
            Role::App(session_id)
        }
        _ => {
            tracing::warn!("WebSocket client failed to authenticate; closing");
            let refusal = Response::Error {
                code: 401,
                message: "authentication required".to_owned(),
            };
            let _ = ws_write
                .send(Message::Text(serde_json::to_string(&refusal)?))
                .await;
            let _ = ws_write.close().await;
            return Ok(());
        }
    };

    match role {
        Role::System => tracing::debug!("system client connected"),
        Role::App(session_id) => tracing::debug!(session_id, "application bridge connected"),
    }

    let mut broadcast_rx = broadcast_rx;
    loop {
        tokio::select! {
            msg = ws_read.next() => {
                let text = match msg {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "ws error");
                        break;
                    }
                };
                let reply = match role {
                    Role::System => system_request(&text, &registry).await,
                    Role::App(session_id) => {
                        app_request(&text, session_id, &registry).await;
                        None
                    }
                };
                if let Some(reply) = reply {
                    ws_write.send(Message::Text(serde_json::to_string(&reply)?)).await?;
                }
            }
            notification = broadcast_rx.recv() => {
                let notification = match notification {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "ws client missed notifications");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                match (&role, notification) {
                    (Role::System, Response::IpcMessage { .. }) => {}
                    (Role::System, other) => {
                        ws_write.send(Message::Text(serde_json::to_string(&other)?)).await?;
                    }
                    (Role::App(bound), Response::IpcMessage { session_id, payload })
                        if *bound == session_id =>
                    {
                        let message = serde_json::json!({ "type": "APP_MESSAGE", "payload": payload });
                        ws_write.send(Message::Text(message.to_string())).await?;
                    }
                    (Role::App(bound), Response::AppState { session_id, state: AppStateKind::Stopped })
                        if *bound == session_id => break,
                    (Role::App(_), _) => {}
                }
            }
        }
    }
    let _ = ws_write.close().await;
    Ok(())
}

/// Dispatches a system-role request. Application messages are not part of
/// the system role's authority.
async fn system_request(text: &str, registry: &Registry) -> Option<Response> {
    let request: Request = match serde_json::from_str(text) {
        Ok(request) => request,
        Err(e) => {
            tracing::debug!(error = %e, "unrecognised system ws message");
            return None;
        }
    };
    if matches!(request, Request::IpcForward { .. }) {
        return Some(Response::Error {
            code: 403,
            message: "application messages are not available to the system role".to_owned(),
        });
    }
    Some(dispatch(request, registry).await)
}

/// Forwards an application message to the bound session's component.
async fn app_request(text: &str, session_id: u64, registry: &Registry) {
    let payload = match serde_json::from_str::<AppRequest>(text) {
        Ok(AppRequest::AppMessage { payload }) => payload,
        Err(e) => {
            tracing::debug!(session_id, error = %e, "unrecognised app ws message");
            return;
        }
    };
    // The component reads newline-delimited messages; a payload is dropped
    // rather than altered when it cannot be framed that way.
    if payload.len() > MAX_APP_PAYLOAD || payload.contains('\n') {
        tracing::warn!(
            session_id,
            len = payload.len(),
            "application payload rejected"
        );
        return;
    }
    let Some(tx) = registry.lock().await.ipc_sender_for(session_id) else {
        tracing::debug!(session_id, "no IPC relay for session; message dropped");
        return;
    };
    // A component that stops reading must not stall this connection, which
    // also has to see its session stop.
    match tx.try_send(payload) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(
                session_id,
                "component is not reading; application message dropped"
            );
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            tracing::warn!(session_id, "IPC relay closed");
            registry.lock().await.remove_ipc_sender(session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionRegistry, ipc::Response};
    use tokio::sync::Mutex;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

    type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    const SYSTEM_TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// Serves one WebSocket connection with `handle_ws_connection`.
    async fn serve(registry: Registry) -> Client {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reg = Arc::clone(&registry);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let rx = reg.lock().await.subscribe();
            let auth = Arc::new(WsAuth {
                system_token: SYSTEM_TOKEN.to_owned(),
            });
            handle_ws_connection(stream, reg, rx, auth).await.unwrap();
        });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        client
    }

    async fn send(client: &mut Client, value: serde_json::Value) {
        client.send(Message::Text(value.to_string())).await.unwrap();
    }

    /// The next text message, or None if the server closed the connection.
    async fn next(client: &mut Client) -> Option<serde_json::Value> {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), client.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => return serde_json::from_str(&text).ok(),
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return None,
                Ok(Some(Ok(_))) => continue,
                Err(_) => panic!("no message within 2 s"),
            }
        }
    }

    /// True if nothing arrives within 300 ms.
    async fn silent(client: &mut Client) -> bool {
        tokio::time::timeout(Duration::from_millis(300), client.next())
            .await
            .is_err()
    }

    fn registry() -> Registry {
        Arc::new(Mutex::new(SessionRegistry::default()))
    }

    /// A session with an IPC relay channel standing in for its component.
    async fn session(registry: &Registry) -> (u64, String, tokio::sync::mpsc::Receiver<String>) {
        let mut reg = registry.lock().await;
        let id = reg.launch("test.app");
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        reg.register_ipc_sender(id, tx);
        let token = reg.bridge_token(id).unwrap();
        (id, token, rx)
    }

    fn refused(message: &Option<serde_json::Value>) -> bool {
        matches!(message, Some(m) if m["type"] == "ERROR" && m["code"] == 401)
    }

    #[tokio::test]
    async fn requests_before_hello_are_refused() {
        let mut client = serve(registry()).await;
        send(&mut client, serde_json::json!({ "type": "QUERY_RUNNING" })).await;
        assert!(refused(&next(&mut client).await));
        assert!(next(&mut client).await.is_none());
    }

    #[tokio::test]
    async fn wrong_system_token_is_refused() {
        let mut client = serve(registry()).await;
        let token = "fedcba9876543210fedcba9876543210";
        send(
            &mut client,
            serde_json::json!({ "type": "HELLO", "role": "system", "token": token }),
        )
        .await;
        assert!(refused(&next(&mut client).await));
    }

    #[tokio::test]
    async fn system_role_controls_sessions_but_not_app_messages() {
        let registry = registry();
        let (id, _, mut component) = session(&registry).await;
        let mut client = serve(Arc::clone(&registry)).await;
        send(
            &mut client,
            serde_json::json!({ "type": "HELLO", "role": "system", "token": SYSTEM_TOKEN }),
        )
        .await;
        send(&mut client, serde_json::json!({ "type": "QUERY_RUNNING" })).await;
        assert_eq!(next(&mut client).await.unwrap()["type"], "RUNNING_APPS");

        send(
            &mut client,
            serde_json::json!({ "type": "IPC_FORWARD", "session_id": id, "payload": "x" }),
        )
        .await;
        assert_eq!(next(&mut client).await.unwrap()["code"], 403);
        assert!(component.try_recv().is_err());

        let broadcast = registry.lock().await.broadcast().clone();
        let _ = broadcast.send(Response::IpcMessage {
            session_id: id,
            payload: "private".to_owned(),
        });
        let _ = broadcast.send(Response::AppState {
            session_id: id,
            state: AppStateKind::Running,
        });
        assert_eq!(next(&mut client).await.unwrap()["type"], "APP_STATE");
    }

    #[tokio::test]
    async fn app_role_reaches_only_its_own_session() {
        let registry = registry();
        let (id, token, mut component) = session(&registry).await;
        let (other, _, mut other_component) = session(&registry).await;
        let mut client = serve(Arc::clone(&registry)).await;
        send(
            &mut client,
            serde_json::json!({ "type": "HELLO", "role": "app", "session_id": id, "token": token }),
        )
        .await;

        send(
            &mut client,
            serde_json::json!({ "type": "APP_MESSAGE", "payload": "increment" }),
        )
        .await;
        let delivered = tokio::time::timeout(Duration::from_secs(2), component.recv()).await;
        assert_eq!(delivered.unwrap().as_deref(), Some("increment"));

        // System requests and other sessions are out of reach.
        send(
            &mut client,
            serde_json::json!({ "type": "TERMINATE_APP", "session_id": other }),
        )
        .await;
        send(
            &mut client,
            serde_json::json!({ "type": "IPC_FORWARD", "session_id": other, "payload": "x" }),
        )
        .await;
        assert!(silent(&mut client).await);
        assert!(other_component.try_recv().is_err());
        assert!(!matches!(
            registry.lock().await.state(other),
            AppStateKind::NotFound
        ));

        let broadcast = registry.lock().await.broadcast().clone();
        let _ = broadcast.send(Response::IpcMessage {
            session_id: other,
            payload: "not yours".to_owned(),
        });
        let _ = broadcast.send(Response::IpcMessage {
            session_id: id,
            payload: "{\"count\":1}".to_owned(),
        });
        let received = next(&mut client).await.unwrap();
        assert_eq!(received["type"], "APP_MESSAGE");
        assert_eq!(received["payload"], "{\"count\":1}");

        let _ = broadcast.send(Response::AppState {
            session_id: id,
            state: AppStateKind::Stopped,
        });
        assert!(next(&mut client).await.is_none());
    }

    #[tokio::test]
    async fn app_token_of_another_or_stopped_session_is_refused() {
        let registry = registry();
        let (id, _, _component) = session(&registry).await;
        let (_, other_token, _other_component) = session(&registry).await;
        let mut client = serve(Arc::clone(&registry)).await;
        send(
            &mut client,
            serde_json::json!({ "type": "HELLO", "role": "app", "session_id": id, "token": other_token }),
        )
        .await;
        assert!(refused(&next(&mut client).await));

        let (stopped, token, _stopped_component) = session(&registry).await;
        registry
            .lock()
            .await
            .set_state(stopped, AppStateKind::Stopped);
        let mut client = serve(Arc::clone(&registry)).await;
        send(
            &mut client,
            serde_json::json!({ "type": "HELLO", "role": "app", "session_id": stopped, "token": token }),
        )
        .await;
        assert!(refused(&next(&mut client).await));
    }

    #[tokio::test]
    async fn app_payloads_that_cannot_be_framed_are_dropped() {
        let registry = registry();
        let (id, token, mut component) = session(&registry).await;
        let mut client = serve(Arc::clone(&registry)).await;
        send(
            &mut client,
            serde_json::json!({ "type": "HELLO", "role": "app", "session_id": id, "token": token }),
        )
        .await;
        send(
            &mut client,
            serde_json::json!({ "type": "APP_MESSAGE", "payload": "a\nb" }),
        )
        .await;
        send(
            &mut client,
            serde_json::json!({ "type": "APP_MESSAGE", "payload": "ok" }),
        )
        .await;
        let delivered = tokio::time::timeout(Duration::from_secs(2), component.recv()).await;
        assert_eq!(delivered.unwrap().as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn stalled_upgrade_is_closed() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reg = registry();
        let handler = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let rx = reg.lock().await.subscribe();
            let auth = Arc::new(WsAuth {
                system_token: SYSTEM_TOKEN.to_owned(),
            });
            handle_ws_connection(stream, reg, rx, auth).await
        });
        // Connects but never sends the HTTP upgrade request.
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let finished = tokio::time::timeout(HELLO_TIMEOUT * 4, handler).await;
        assert!(finished.expect("handler still waiting").unwrap().is_ok());
        let mut byte = [0u8; 1];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn component_that_stops_reading_does_not_stall_the_bridge() {
        let registry = registry();
        let id = {
            let mut reg = registry.lock().await;
            let id = reg.launch("test.app");
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            // The receiver stays alive but is never read.
            std::mem::forget(rx);
            reg.register_ipc_sender(id, tx);
            id
        };
        let token = registry.lock().await.bridge_token(id).unwrap();
        let mut client = serve(Arc::clone(&registry)).await;
        send(
            &mut client,
            serde_json::json!({ "type": "HELLO", "role": "app", "session_id": id, "token": token }),
        )
        .await;
        for _ in 0..4 {
            send(
                &mut client,
                serde_json::json!({ "type": "APP_MESSAGE", "payload": "increment" }),
            )
            .await;
        }
        let broadcast = registry.lock().await.broadcast().clone();
        let _ = broadcast.send(Response::IpcMessage {
            session_id: id,
            payload: "still here".to_owned(),
        });
        assert_eq!(next(&mut client).await.unwrap()["payload"], "still here");
        let _ = broadcast.send(Response::AppState {
            session_id: id,
            state: AppStateKind::Stopped,
        });
        assert!(next(&mut client).await.is_none());
    }

    #[test]
    fn tokens_match_requires_equal_length_and_bytes() {
        assert!(tokens_match("abc", "abc"));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", "abcd"));
        assert!(!tokens_match("abc", ""));
    }
}
