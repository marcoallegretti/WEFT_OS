#![cfg(feature = "servo-embed")]

use std::sync::mpsc;

pub enum AppdCmd {
    Launch { session_id: u64, app_id: String },
    Stop { session_id: u64 },
}

pub fn spawn_appd_listener(
    ws_port: u16,
    tx: mpsc::SyncSender<AppdCmd>,
    wake: Box<dyn Fn() + Send>,
) {
    std::thread::Builder::new()
        .name("appd-ws".into())
        .spawn(move || run_listener(ws_port, tx, wake))
        .ok();
}

fn run_listener(ws_port: u16, tx: mpsc::SyncSender<AppdCmd>, wake: Box<dyn Fn() + Send>) {
    let url = format!("ws://127.0.0.1:{ws_port}");
    let (mut ws, _) = match tungstenite::connect(url) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("appd WebSocket connect failed: {e}");
            return;
        }
    };

    let _ = ws.send(tungstenite::Message::Text(
        r#"{"type":"QUERY_RUNNING"}"#.into(),
    ));

    loop {
        match ws.read() {
            Ok(tungstenite::Message::Text(text)) => {
                process_message(&text, &tx, &*wake);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

fn process_message(text: &str, tx: &mpsc::SyncSender<AppdCmd>, wake: &dyn Fn()) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    match v["type"].as_str() {
        Some("LAUNCH_ACK") => {
            let Some(session_id) = v["session_id"].as_u64() else {
                return;
            };
            let Some(app_id) = v["app_id"].as_str().map(str::to_string) else {
                return;
            };
            if tx.try_send(AppdCmd::Launch { session_id, app_id }).is_ok() {
                wake();
            }
        }
        Some("RUNNING_APPS") => {
            let Some(sessions) = v["sessions"].as_array() else {
                return;
            };
            for s in sessions {
                let Some(session_id) = s["session_id"].as_u64() else {
                    continue;
                };
                let Some(app_id) = s["app_id"].as_str().map(str::to_string) else {
                    continue;
                };
                let _ = tx.try_send(AppdCmd::Launch { session_id, app_id });
            }
            wake();
        }
        Some("APP_STATE") if v["state"].as_str() == Some("stopped") => {
            let Some(session_id) = v["session_id"].as_u64() else {
                return;
            };
            if tx.try_send(AppdCmd::Stop { session_id }).is_ok() {
                wake();
            }
        }
        _ => {}
    }
}
