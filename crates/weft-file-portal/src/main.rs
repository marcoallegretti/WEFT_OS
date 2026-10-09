#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use anyhow::Context;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Read { path: String },
    Write { path: String, data_b64: String },
    List { path: String },
}

#[derive(Serialize)]
#[serde(untagged)]
enum Response {
    Ok,
    OkData { data_b64: String },
    OkEntries { entries: Vec<String> },
    Err { error: String },
}

impl Response {
    fn err(msg: impl std::fmt::Display) -> Self {
        Self::Err {
            error: msg.to_string(),
        }
    }
}

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: weft-file-portal <socket_path> [--allow <path>]... [--allow-read <path>]..."
        );
        std::process::exit(1);
    }

    let socket_path = &args[1];
    let allowed = Arc::new(parse_allowed(&args[2..]));

    if Path::new(socket_path).exists() {
        std::fs::remove_file(socket_path)
            .with_context(|| format!("remove stale socket {socket_path}"))?;
    }

    let listener =
        UnixListener::bind(socket_path).with_context(|| format!("bind {socket_path}"))?;

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let allowed = Arc::clone(&allowed);
                std::thread::spawn(move || handle_connection(s, &allowed));
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }

    Ok(())
}

#[cfg(not(unix))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("weft-file-portal requires a Unix platform")
}

/// A directory the session may use: `--allow` grants reading and writing,
/// `--allow-read` reading only.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Allowed {
    root: PathBuf,
    writable: bool,
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn parse_allowed(args: &[String]) -> Vec<Allowed> {
    let mut allowed = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let writable = match args[i].as_str() {
            "--allow" => true,
            "--allow-read" => false,
            _ => {
                i += 1;
                continue;
            }
        };
        if let Some(p) = args.get(i + 1) {
            allowed.push(Allowed {
                root: PathBuf::from(p),
                writable,
            });
        }
        i += 2;
    }
    allowed
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Whether `path` lies in an allowed directory, and for a write, in one
/// granted read-write.
fn is_allowed(path: &Path, allowed: &[Allowed], write: bool) -> bool {
    let norm = normalize_path(path);
    allowed
        .iter()
        .any(|a| norm.starts_with(&a.root) && (a.writable || !write))
}

#[cfg(unix)]
fn handle_connection(stream: UnixStream, allowed: &[Allowed]) {
    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stream clone error: {e}");
            return;
        }
    };
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle_request(req, allowed),
            Err(e) => Response::err(format!("bad request: {e}")),
        };

        let mut out = serde_json::to_string(&response)
            .unwrap_or_else(|_| r#"{"error":"serialize"}"#.to_string());
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
    }
}

#[cfg_attr(not(any(unix, test)), allow(dead_code))]
fn handle_request(req: Request, allowed: &[Allowed]) -> Response {
    match req {
        Request::Read { path } => {
            let p = PathBuf::from(&path);
            if !is_allowed(&p, allowed, false) {
                return Response::err(format!("access denied: {path}"));
            }
            match std::fs::read(&p) {
                Ok(data) => Response::OkData {
                    data_b64: base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &data,
                    ),
                },
                Err(e) => Response::err(e),
            }
        }
        Request::Write { path, data_b64 } => {
            let p = PathBuf::from(&path);
            if !is_allowed(&p, allowed, true) {
                return Response::err(format!("access denied: {path}"));
            }
            let data =
                match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data_b64)
                {
                    Ok(d) => d,
                    Err(e) => return Response::err(format!("bad base64: {e}")),
                };
            if let Some(Err(e)) = p.parent().map(std::fs::create_dir_all) {
                return Response::err(e);
            }
            match std::fs::write(&p, &data) {
                Ok(()) => Response::Ok,
                Err(e) => Response::err(e),
            }
        }
        Request::List { path } => {
            let p = PathBuf::from(&path);
            if !is_allowed(&p, allowed, false) {
                return Response::err(format!("access denied: {path}"));
            }
            match std::fs::read_dir(&p) {
                Ok(entries) => {
                    let mut names = Vec::new();
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            names.push(name.to_string());
                        }
                    }
                    names.sort();
                    Response::OkEntries { entries: names }
                }
                Err(e) => Response::err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(root: impl Into<PathBuf>) -> Allowed {
        Allowed {
            root: root.into(),
            writable: true,
        }
    }

    fn ro(root: impl Into<PathBuf>) -> Allowed {
        Allowed {
            root: root.into(),
            writable: false,
        }
    }

    #[test]
    fn allowed_path_accepted() {
        let allowed = vec![rw("/tmp/weft-test-allowed")];
        assert!(is_allowed(
            Path::new("/tmp/weft-test-allowed/file.txt"),
            &allowed,
            false
        ));
    }

    #[test]
    fn disallowed_path_rejected() {
        let allowed = vec![rw("/tmp/weft-test-allowed")];
        assert!(!is_allowed(Path::new("/etc/passwd"), &allowed, false));
    }

    #[test]
    fn dotdot_traversal_blocked() {
        let allowed = vec![rw("/tmp/weft-test-allowed")];
        assert!(!is_allowed(
            Path::new("/tmp/weft-test-allowed/../etc/passwd"),
            &allowed,
            false
        ));
    }

    #[test]
    fn empty_allowlist_rejects_all() {
        assert!(!is_allowed(Path::new("/tmp/anything"), &[], false));
    }

    #[test]
    fn parse_allowed_extracts_paths() {
        let args: Vec<String> = vec![
            "--allow".into(),
            "/tmp/a".into(),
            "--allow".into(),
            "/tmp/b".into(),
        ];
        let result = parse_allowed(&args);
        assert_eq!(result, vec![rw("/tmp/a"), rw("/tmp/b")]);
        let args: Vec<String> = vec!["--allow-read".into(), "/tmp/c".into()];
        assert_eq!(parse_allowed(&args), vec![ro("/tmp/c")]);
    }

    #[test]
    fn handle_request_read_denied() {
        let resp = handle_request(
            Request::Read {
                path: "/etc/shadow".into(),
            },
            &[rw("/tmp/safe")],
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("access denied"));
    }

    #[test]
    fn handle_request_read_roundtrip() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("wfp_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("hello.txt");
        fs::write(&file, b"hello world").unwrap();

        let allowed = vec![rw(dir.clone())];
        let resp = handle_request(
            Request::Read {
                path: file.to_string_lossy().into(),
            },
            &allowed,
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("data_b64"));

        if let Response::OkData { data_b64 } = resp {
            let decoded =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data_b64)
                    .unwrap();
            assert_eq!(decoded, b"hello world");
        } else {
            panic!("expected OkData");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_request_list() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("wfp_list_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("b.txt"), b"").unwrap();
        fs::write(dir.join("a.txt"), b"").unwrap();

        let allowed = vec![rw(dir.clone())];
        let resp = handle_request(
            Request::List {
                path: dir.to_string_lossy().into(),
            },
            &allowed,
        );
        if let Response::OkEntries { entries } = resp {
            assert_eq!(entries, vec!["a.txt", "b.txt"]);
        } else {
            panic!("expected OkEntries");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_request_write_creates_parent_dirs() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("wfp_write_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let nested = dir.join("sub").join("deep").join("file.txt");
        let data = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            b"nested content",
        );
        let allowed = vec![rw(dir.clone())];
        let resp = handle_request(
            Request::Write {
                path: nested.to_string_lossy().into(),
                data_b64: data,
            },
            &allowed,
        );
        assert!(matches!(resp, Response::Ok), "expected Ok response");
        assert_eq!(fs::read(&nested).unwrap(), b"nested content");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_directories_refuse_writes() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("wfp_ro_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("kept.txt"), b"original").unwrap();
        let allowed = vec![ro(dir.clone())];
        let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"x");
        let resp = handle_request(
            Request::Write {
                path: dir.join("kept.txt").to_string_lossy().into(),
                data_b64: data,
            },
            &allowed,
        );
        assert!(
            serde_json::to_string(&resp)
                .unwrap()
                .contains("access denied")
        );
        assert_eq!(fs::read(dir.join("kept.txt")).unwrap(), b"original");
        let resp = handle_request(
            Request::Read {
                path: dir.join("kept.txt").to_string_lossy().into(),
            },
            &allowed,
        );
        assert!(matches!(resp, Response::OkData { .. }));
        let _ = fs::remove_dir_all(&dir);
    }
}
