wit_bindgen::generate!({
    path: "wit",
    world: "app",
    with: {
        "weft:app/notify@0.1.0": generate,
        "weft:app/ipc@0.1.0": generate,
    },
});

use serde_json::{Value, json};
use weft::app::{ipc, notify};

const NOTES_PATH: &str = "/data/notes.txt";
const STAGING_PATH: &str = "/data/.notes.txt.saving";
/// Largest note accepted, in bytes; app messages are limited to 64 KiB.
const MAX_NOTES: usize = 60 * 1024;

/// A revision names the exact stored bytes. A save must name the revision
/// it was edited from, so a save based on stale content is refused instead
/// of silently replacing changes made in another window.
fn revision(bytes: &[u8]) -> String {
    // FNV-1a, 64-bit.
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

fn stored() -> Result<Vec<u8>, String> {
    match std::fs::read(NOTES_PATH) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("cannot read notes: {e}")),
    }
}

fn load() -> Value {
    let bytes = match stored() {
        Ok(bytes) => bytes,
        Err(message) => return json!({ "op": "error", "message": message }),
    };
    match String::from_utf8(bytes) {
        Ok(text) => json!({ "op": "loaded", "rev": revision(text.as_bytes()), "text": text }),
        Err(_) => json!({ "op": "error", "message": "stored notes are not valid UTF-8" }),
    }
}

fn save(seq: &Value, base: &str, text: &str) -> Value {
    let error = |conflict: bool, message: String| {
        json!({ "op": "error", "seq": seq, "conflict": conflict, "message": message })
    };
    if text.len() > MAX_NOTES {
        return error(false, format!("notes are limited to {MAX_NOTES} bytes"));
    }
    let current = match stored() {
        Ok(bytes) => revision(&bytes),
        Err(message) => return error(false, message),
    };
    if current != base {
        return error(true, "the notes were changed elsewhere".to_owned());
    }
    // Written beside the file and renamed over it, so the stored notes are
    // always either the previous or the new text in full.
    let written = std::fs::write(STAGING_PATH, text.as_bytes())
        .and_then(|()| std::fs::rename(STAGING_PATH, NOTES_PATH));
    match written {
        Ok(()) => json!({ "op": "saved", "seq": seq, "rev": revision(text.as_bytes()) }),
        Err(e) => {
            let _ = std::fs::remove_file(STAGING_PATH);
            error(false, format!("cannot save notes: {e}"))
        }
    }
}

fn handle(raw: &str) -> Option<Value> {
    let request: Value = serde_json::from_str(raw).ok()?;
    match request.get("op")?.as_str()? {
        "load" => Some(load()),
        "save" => {
            let base = request.get("base")?.as_str()?;
            let text = request.get("text")?.as_str()?;
            Some(save(request.get("seq").unwrap_or(&Value::Null), base, text))
        }
        _ => None,
    }
}

fn main() {
    notify::ready();

    loop {
        if let Some(raw) = ipc::recv() {
            if let Some(reply) = handle(&raw) {
                let _ = ipc::send(&reply.to_string());
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
