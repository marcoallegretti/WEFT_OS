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

/// A value unique to this process and moment, for lock ownership and
/// staging file names.
fn unique() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{nanos}-{n}")
}

/// Exclusive access to the notes file for one save. The lock file holds an
/// owner token, so a save only ever removes its own lock.
struct SaveLock {
    token: String,
}

impl SaveLock {
    fn acquire() -> Result<Self, String> {
        use std::io::Write;
        let token = unique();
        for _ in 0..50 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(LOCK_PATH)
            {
                Ok(mut file) => {
                    let _ = file.write_all(token.as_bytes());
                    return Ok(SaveLock { token });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // A lock older than STALE_LOCK (or of unknown age) was left
                    // by an ended session. Renaming it away is atomic, so only
                    // one save can take it over.
                    let stale = std::fs::metadata(LOCK_PATH)
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().is_ok_and(|age| age > STALE_LOCK))
                        .unwrap_or(true);
                    let taken = format!("/data/.notes.lock.{token}.stale");
                    if stale && std::fs::rename(LOCK_PATH, &taken).is_ok() {
                        let _ = std::fs::remove_file(&taken);
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
        if std::fs::read_to_string(LOCK_PATH).is_ok_and(|t| t == self.token) {
            let _ = std::fs::remove_file(LOCK_PATH);
        }
    }
}

/// Removes staging files left by saves that ended before their rename. Only
/// called with the lock held, when no other save is in progress.
fn remove_abandoned_staging() {
    let Ok(entries) = std::fs::read_dir("/data") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".notes.txt.") && name.ends_with(".saving") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Writes `text` to a staging file unique to this save, syncs it and renames
/// it over the notes, so the stored notes are always either the previous or
/// the new text in full.
fn replace_notes(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let staging = format!("/data/.notes.txt.{}.saving", unique());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&staging, NOTES_PATH));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    written
}

fn save(seq: &Value, base: &str, text: &str) -> Value {
    let error = |conflict: bool, message: String| json!({ "op": "error", "seq": seq, "conflict": conflict, "message": message });
    if text.len() > MAX_NOTES {
        return error(false, format!("notes are limited to {MAX_NOTES} bytes"));
    }
    let _lock = match SaveLock::acquire() {
        Ok(lock) => lock,
        Err(message) => return error(false, message),
    };
    remove_abandoned_staging();
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
