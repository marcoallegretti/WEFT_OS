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
/// Held while a save checks the revision and replaces the file, so saves
/// from two Notes windows cannot interleave.
const LOCK_PATH: &str = "/data/.notes.lock";
/// A lock older than this was left by a session that ended mid-save.
const STALE_LOCK: std::time::Duration = std::time::Duration::from_secs(10);
/// Largest note accepted, in bytes; the page also keeps each request under
/// the 64 KiB app message limit.
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

/// Exclusive access to the notes file for one save.
struct SaveLock;

impl SaveLock {
    fn acquire() -> Result<Self, String> {
        for _ in 0..50 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(LOCK_PATH) {
                Ok(_) => return Ok(SaveLock),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(LOCK_PATH)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE_LOCK);
                    if stale {
                        let _ = std::fs::remove_file(LOCK_PATH);
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
                Err(e) => return Err(format!("cannot lock notes: {e}")),
            }
        }
        Err("notes are being saved by another window; try again".to_owned())
    }
}

impl Drop for SaveLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(LOCK_PATH);
    }
}

/// Writes `text` to a staging file unique to this save, syncs it and renames
/// it over the notes, so the stored notes are always either the previous or
/// the new text in full.
fn replace_notes(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let staging = format!("/data/.notes.txt.{nanos}.saving");
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&staging, NOTES_PATH));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    written
}

fn save(seq: &Value, base: &str, text: &str) -> Value {
    let error = |conflict: bool, message: String| {
        json!({ "op": "error", "seq": seq, "conflict": conflict, "message": message })
    };
    if text.len() > MAX_NOTES {
        return error(false, format!("notes are limited to {MAX_NOTES} bytes"));
    }
    let _lock = match SaveLock::acquire() {
        Ok(lock) => lock,
        Err(message) => return error(false, message),
    };
    let current = match stored() {
        Ok(bytes) => revision(&bytes),
        Err(message) => return error(false, message),
    };
    if current != base {
        return error(true, "the notes were changed elsewhere".to_owned());
    }
    match replace_notes(text) {
        Ok(()) => json!({ "op": "saved", "seq": seq, "rev": revision(text.as_bytes()) }),
        Err(e) => error(false, format!("cannot save notes: {e}")),
    }
}

/// Every request gets a reply, so the page never waits on one that was not
/// understood.
fn handle(raw: &str) -> Value {
    let Ok(request) = serde_json::from_str::<Value>(raw) else {
        return json!({ "op": "error", "message": "request is not valid JSON" });
    };
    let seq = request.get("seq").cloned().unwrap_or(Value::Null);
    let field = |name: &str| request.get(name).and_then(Value::as_str);
    match field("op") {
        Some("load") => load(),
        Some("save") => match (field("base"), field("text")) {
            (Some(base), Some(text)) => save(&seq, base, text),
            _ => json!({ "op": "error", "seq": seq, "message": "malformed save request" }),
        },
        _ => json!({ "op": "error", "seq": seq, "message": "unknown request" }),
    }
}

fn main() {
    notify::ready();

    loop {
        if let Some(raw) = ipc::recv() {
            let _ = ipc::send(&handle(&raw).to_string());
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
