wit_bindgen::generate!({
    path: "wit",
    world: "app",
    with: {
        "weft:app/notify@0.1.0": generate,
        "weft:app/ipc@0.1.0": generate,
    },
});

use weft::app::{ipc, notify};

const NOTES_PATH: &str = "/data/notes.txt";

fn load_notes() -> String {
    std::fs::read_to_string(NOTES_PATH).unwrap_or_default()
}

fn save_notes(text: &str) -> Result<(), String> {
    std::fs::write(NOTES_PATH, text).map_err(|e| e.to_string())
}

fn json_text(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("{{\"text\":\"{escaped}\"}}")
}

fn json_error(msg: &str) -> String {
    let escaped = msg.replace('"', "\\\"");
    format!("{{\"error\":\"{escaped}\"}}")
}

fn main() {
    notify::ready();

    loop {
        if let Some(raw) = ipc::recv() {
            let raw = raw.trim().to_owned();
            let reply = if raw == "load" {
                json_text(&load_notes())
            } else if let Some(rest) = raw.strip_prefix("save:") {
                let text = rest.replace("\\n", "\n");
                match save_notes(&text) {
                    Ok(()) => json_text(&text),
                    Err(e) => json_error(&e),
                }
            } else {
                continue;
            };
            let _ = ipc::send(&reply);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
