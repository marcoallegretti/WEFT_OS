//! A per-session file proxy for the directories a session is granted.
//!
//! Requests name absolute host paths inside a granted directory. Each
//! granted directory is opened once at startup, and every operation resolves
//! the rest of the path relative to that descriptor with `openat2` and
//! `RESOLVE_BENEATH`, so the kernel keeps it inside the directory while it
//! runs: `..`, absolute symbolic links, links that point outside and links
//! replaced during the operation cannot leave it. Paths must name their
//! location directly, without `.` or `..` components. Writes replace a file
//! atomically and never create directories.

#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The largest file read or written, in bytes.
const MAX_FILE: usize = 16 * 1024 * 1024;
/// The longest request line: a write of the largest file, base64-encoded,
/// with room for its path.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MAX_LINE: usize = MAX_FILE / 3 * 4 + 64 * 1024;
/// The most entries one listing returns.
const MAX_ENTRIES: usize = 10_000;
/// The most connections served at once.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MAX_CONNECTIONS: usize = 4;
/// How long a connection may stay silent before it is closed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// The prefix of the temporary files writes create, never listed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const TEMPORARY_PREFIX: &str = ".weft-portal-";

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

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "usage: weft-file-portal <socket_path> [--allow <path>]... [--allow-read <path>]..."
        );
        std::process::exit(1);
    }

    let socket_path = &args[1];
    let mut roots = Vec::new();
    for grant in parse_allowed(&args[2..]) {
        match Root::open(grant) {
            Ok(root) => roots.push(root),
            Err((path, e)) => eprintln!("cannot open granted directory {}: {e}", path.display()),
        }
    }
    let roots = Arc::new(roots);

    if Path::new(socket_path).exists() {
        std::fs::remove_file(socket_path)
            .with_context(|| format!("remove stale socket {socket_path}"))?;
    }
    let listener =
        UnixListener::bind(socket_path).with_context(|| format!("bind {socket_path}"))?;

    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            let _ = stream.write_all(b"{\"error\":\"too many connections\"}\n");
            continue;
        }
        // The slot is released however the connection ends, a panic
        // included.
        struct Slot(Arc<AtomicUsize>);
        impl Drop for Slot {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let slot = Slot(Arc::clone(&active));
        let roots = Arc::clone(&roots);
        if let Err(e) = std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            handle_connection(stream, &roots);
        }) {
            eprintln!("cannot serve a connection: {e}");
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("weft-file-portal requires Linux")
}

/// A directory the session may use: `--allow` grants reading and writing,
/// `--allow-read` reading only.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Allowed {
    root: PathBuf,
    writable: bool,
}

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

/// The part of `path` inside the granted directory `root`, if `path` lies
/// in it and names its location directly: an absolute path of plain
/// components only, without `.`, `..`, empty components or a trailing `/`.
/// The text is checked as given, since path parsing drops `.` and repeated
/// separators.
fn relative_to<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    let text = path.to_str()?;
    let direct = text.starts_with('/')
        && text[1..]
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."));
    if !direct {
        return None;
    }
    let rest = path.strip_prefix(root).ok()?;
    rest.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then_some(rest)
}

/// A granted directory, held open for the life of the portal.
#[cfg(target_os = "linux")]
struct Root {
    path: PathBuf,
    dir: rustix::fd::OwnedFd,
    writable: bool,
}

#[cfg(target_os = "linux")]
impl Root {
    fn open(grant: Allowed) -> Result<Self, (PathBuf, std::io::Error)> {
        use rustix::fs::{Mode, OFlags};
        match rustix::fs::open(
            &grant.root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(dir) => Ok(Self {
                path: grant.root,
                dir,
                writable: grant.writable,
            }),
            Err(e) => Err((grant.root, e.into())),
        }
    }
}

/// The granted directory holding `path` and the path inside it, for an
/// operation that writes when `write` is set.
#[cfg(target_os = "linux")]
fn locate<'a, 'p>(
    roots: &'a [Root],
    path: &'p str,
    write: bool,
) -> Result<(&'a Root, &'p Path), Response> {
    let requested = Path::new(path);
    roots
        .iter()
        .filter(|root| root.writable || !write)
        .find_map(|root| relative_to(requested, &root.path).map(|rest| (root, rest)))
        .ok_or_else(|| Response::err(format!("access denied: {path}")))
}

/// Opens `rest` inside `root`, never leaving it.
#[cfg(target_os = "linux")]
fn open_beneath(
    root: &Root,
    rest: &Path,
    flags: rustix::fs::OFlags,
) -> std::io::Result<rustix::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    let rest = if rest.as_os_str().is_empty() {
        Path::new(".")
    } else {
        rest
    };
    rustix::fs::openat2(
        &root.dir,
        rest,
        flags | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|e| {
        // The kernel reports a resolution that would leave the directory as
        // EXDEV.
        if e == rustix::io::Errno::XDEV {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "access denied: the path leads out of the granted directory",
            )
        } else {
            e.into()
        }
    })
}

#[cfg(target_os = "linux")]
fn handle_connection(stream: std::os::unix::net::UnixStream, roots: &[Root]) {
    let mut writer = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stream clone error: {e}");
            return;
        }
    };
    let _ = stream.set_read_timeout(Some(IDLE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IDLE_TIMEOUT));
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader)
            .take(MAX_LINE as u64 + 3)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let content = line.strip_suffix(b"\n").unwrap_or(&line);
        if content.strip_suffix(b"\r").unwrap_or(content).len() > MAX_LINE {
            let _ = writer.write_all(b"{\"error\":\"request too long\"}\n");
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(text) {
            Ok(req) => handle_request(req, roots),
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

#[cfg(target_os = "linux")]
fn handle_request(req: Request, roots: &[Root]) -> Response {
    let result = match req {
        Request::Read { path } => {
            locate(roots, &path, false).and_then(|(root, rest)| read(root, rest))
        }
        Request::Write { path, data_b64 } => locate(roots, &path, true).and_then(|(root, rest)| {
            let data =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data_b64)
                    .map_err(|e| Response::err(format!("bad base64: {e}")))?;
            write(root, rest, &data)
        }),
        Request::List { path } => {
            locate(roots, &path, false).and_then(|(root, rest)| list(root, rest))
        }
    };
    result.unwrap_or_else(|response| response)
}

#[cfg(target_os = "linux")]
fn read(root: &Root, rest: &Path) -> Result<Response, Response> {
    use rustix::fs::{FileType, OFlags};
    let fd = open_beneath(root, rest, OFlags::RDONLY | OFlags::NONBLOCK).map_err(Response::err)?;
    let stat = rustix::fs::fstat(&fd).map_err(Response::err)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Response::err("not a regular file"));
    }
    let mut data = Vec::new();
    std::fs::File::from(fd)
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut data)
        .map_err(Response::err)?;
    if data.len() > MAX_FILE {
        return Err(Response::err(format!("file larger than {MAX_FILE} bytes")));
    }
    Ok(Response::OkData {
        data_b64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data),
    })
}

#[cfg(target_os = "linux")]
fn list(root: &Root, rest: &Path) -> Result<Response, Response> {
    use rustix::fs::OFlags;
    let fd = open_beneath(root, rest, OFlags::RDONLY | OFlags::DIRECTORY).map_err(Response::err)?;
    let mut entries = rustix::fs::Dir::read_from(&fd).map_err(Response::err)?;
    let mut names = Vec::new();
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(Response::err)?;
        let Ok(name) = entry.file_name().to_str() else {
            continue;
        };
        if name == "." || name == ".." || name.starts_with(TEMPORARY_PREFIX) {
            continue;
        }
        if names.len() == MAX_ENTRIES {
            return Err(Response::err(format!("more than {MAX_ENTRIES} entries")));
        }
        names.push(name.to_owned());
    }
    names.sort();
    Ok(Response::OkEntries { entries: names })
}

/// Replaces the file `rest` in `root` with `data`: the data is written to a
/// new file in the same directory, flushed and renamed over the name, so
/// the file holds either the old or the new content.
#[cfg(target_os = "linux")]
fn write(root: &Root, rest: &Path, data: &[u8]) -> Result<Response, Response> {
    use rustix::fs::{Mode, OFlags};
    if data.len() > MAX_FILE {
        return Err(Response::err(format!("file larger than {MAX_FILE} bytes")));
    }
    let name = rest
        .file_name()
        .ok_or_else(|| Response::err("a write needs a file name"))?;
    let parent = open_beneath(
        root,
        rest.parent().unwrap_or(Path::new("")),
        OFlags::RDONLY | OFlags::DIRECTORY,
    )
    .map_err(Response::err)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Unique within this portal by the counter, and across restarts by the
    // time of its first write, so a file left by an earlier instance does
    // not collide.
    static STARTED: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    let started = STARTED.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    });
    let temporary = format!(
        "{TEMPORARY_PREFIX}{}-{started}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    // A replaced file keeps its permission bits; a new one is private.
    let mode = match rustix::fs::statat(&parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat)
            if rustix::fs::FileType::from_raw_mode(stat.st_mode)
                == rustix::fs::FileType::RegularFile =>
        {
            stat.st_mode & 0o777
        }
        _ => 0o600,
    };
    let file = rustix::fs::openat(
        &parent,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(Response::err)?;
    let _ = rustix::fs::fchmod(&file, Mode::from_raw_mode(mode));
    let mut file = std::fs::File::from(file);
    let written = file
        .write_all(data)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            rustix::fs::renameat(&parent, temporary.as_str(), &parent, name)
                .map_err(std::io::Error::from)
        });
    if let Err(e) = written {
        let _ = rustix::fs::unlinkat(&parent, temporary.as_str(), rustix::fs::AtFlags::empty());
        return Err(Response::err(e));
    }
    let _ = rustix::fs::fsync(&parent);
    Ok(Response::Ok)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;

    fn grant(root: &Path, writable: bool) -> Vec<Root> {
        vec![
            Root::open(Allowed {
                root: root.to_path_buf(),
                writable,
            })
            .unwrap(),
        ]
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wfp_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("granted")).unwrap();
        dir
    }

    fn path(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn denied(resp: &Response) -> bool {
        matches!(resp, Response::Err { .. })
    }

    fn read_req(p: &Path) -> Request {
        Request::Read { path: path(p) }
    }

    fn write_req(p: &Path, data: &[u8]) -> Request {
        Request::Write {
            path: path(p),
            data_b64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
        }
    }

    #[test]
    fn parse_allowed_extracts_paths() {
        let args: Vec<String> = vec![
            "--allow".into(),
            "/tmp/a".into(),
            "--allow-read".into(),
            "/tmp/c".into(),
        ];
        assert_eq!(
            parse_allowed(&args),
            vec![
                Allowed {
                    root: "/tmp/a".into(),
                    writable: true
                },
                Allowed {
                    root: "/tmp/c".into(),
                    writable: false
                },
            ]
        );
    }

    #[test]
    fn paths_must_name_a_place_inside_a_granted_directory_directly() {
        let root = Path::new("/srv/granted");
        assert_eq!(
            relative_to(Path::new("/srv/granted/a/b"), root),
            Some(Path::new("a/b"))
        );
        assert_eq!(
            relative_to(Path::new("/srv/granted"), root),
            Some(Path::new(""))
        );
        for outside in [
            "/srv/grantedx/a",
            "/srv/granted/../x",
            "/srv/granted/a/../../x",
            "/etc",
            "/srv/granted/a/./b",
            "/srv/granted/a/.",
            "/srv/granted/a/",
            "/srv/granted//a",
            "srv/granted/a",
        ] {
            assert_eq!(relative_to(Path::new(outside), root), None, "{outside}");
        }
    }

    #[test]
    fn reads_writes_and_lists_inside_the_directory() {
        let dir = temp("roundtrip");
        let granted = dir.join("granted");
        let roots = grant(&granted, true);
        assert!(matches!(
            handle_request(write_req(&granted.join("b.txt"), b"hello"), &roots),
            Response::Ok
        ));
        fs::write(granted.join("a.txt"), b"").unwrap();
        match handle_request(read_req(&granted.join("b.txt")), &roots) {
            Response::OkData { data_b64 } => assert_eq!(
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data_b64)
                    .unwrap(),
                b"hello"
            ),
            _ => panic!("expected data"),
        }
        match handle_request(
            Request::List {
                path: path(&granted),
            },
            &roots,
        ) {
            Response::OkEntries { entries } => assert_eq!(entries, ["a.txt", "b.txt"]),
            _ => panic!("expected entries"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_cannot_lead_out_of_the_directory() {
        let dir = temp("links");
        let granted = dir.join("granted");
        fs::write(dir.join("secret.txt"), b"secret").unwrap();
        fs::create_dir(granted.join("sub")).unwrap();
        std::os::unix::fs::symlink("/", granted.join("root")).unwrap();
        std::os::unix::fs::symlink("../secret.txt", granted.join("up")).unwrap();
        std::os::unix::fs::symlink(&dir, granted.join("parent")).unwrap();
        let roots = grant(&granted, true);
        for escape in [
            granted.join("root/etc/passwd"),
            granted.join("up"),
            granted.join("parent/secret.txt"),
            // The check this replaced normalised `link/..` away and then
            // followed the link.
            granted.join("root/../secret.txt"),
        ] {
            assert!(
                denied(&handle_request(read_req(&escape), &roots)),
                "{}",
                escape.display()
            );
            if escape != granted.join("up") {
                assert!(
                    denied(&handle_request(write_req(&escape, b"x"), &roots)),
                    "{}",
                    escape.display()
                );
            }
        }
        // A write to a name that is a link replaces the link, inside the
        // directory, rather than writing where it points.
        match handle_request(read_req(&granted.join("root/etc/passwd")), &roots) {
            Response::Err { error } => assert!(error.contains("leads out"), "{error}"),
            _ => panic!("read through a link out of the directory"),
        }
        assert!(matches!(
            handle_request(write_req(&granted.join("up"), b"x"), &roots),
            Response::Ok
        ));
        assert!(fs::symlink_metadata(granted.join("up")).unwrap().is_file());
        assert_eq!(fs::read(dir.join("secret.txt")).unwrap(), b"secret");
        // A link that stays inside is followed.
        std::os::unix::fs::symlink("sub", granted.join("inside")).unwrap();
        assert!(matches!(
            handle_request(write_req(&granted.join("inside/f"), b"ok"), &roots),
            Response::Ok
        ));
        assert_eq!(fs::read(granted.join("sub/f")).unwrap(), b"ok");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_swapped_for_a_link_mid_operation_cannot_lead_out() {
        let dir = temp("race");
        let granted = dir.join("granted");
        fs::create_dir(dir.join("outside")).unwrap();
        fs::write(dir.join("outside/f"), b"secret").unwrap();
        fs::create_dir(granted.join("real")).unwrap();
        fs::write(granted.join("real/f"), b"inside").unwrap();
        let roots = grant(&granted, true);
        // `swap` is a link, replaced in a loop, alternately to the inside
        // directory and out of the granted one, absolute and relative.
        std::os::unix::fs::symlink("real", granted.join("swap")).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let swapper = {
            let (granted, outside, stop) = (granted.clone(), dir.join("outside"), stop.clone());
            std::thread::spawn(move || {
                let targets = [PathBuf::from("real"), outside, PathBuf::from("../outside")];
                for target in targets.iter().cycle() {
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let _ = std::os::unix::fs::symlink(target, granted.join("next"));
                    let _ = fs::rename(granted.join("next"), granted.join("swap"));
                }
            })
        };
        let (mut inside, mut refused) = (0, 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while (inside < 50 || refused < 50) && std::time::Instant::now() < deadline {
            match handle_request(read_req(&granted.join("swap/f")), &roots) {
                Response::OkData { data_b64 } => {
                    let data = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        data_b64,
                    )
                    .unwrap();
                    assert_eq!(data, b"inside", "a read left the directory");
                    inside += 1;
                }
                Response::Err { error } if error.contains("leads out") => refused += 1,
                _ => {}
            }
            let _ = handle_request(write_req(&granted.join("swap/f"), b"inside"), &roots);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        swapper.join().unwrap();
        assert_eq!(fs::read(dir.join("outside/f")).unwrap(), b"secret");
        assert!(
            inside >= 50 && refused >= 50,
            "the race was not exercised: {inside} reads inside, {refused} refused"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_replaced_file_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("mode");
        let granted = dir.join("granted");
        fs::write(granted.join("shared.txt"), b"old").unwrap();
        fs::set_permissions(
            granted.join("shared.txt"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let roots = grant(&granted, true);
        assert!(matches!(
            handle_request(write_req(&granted.join("shared.txt"), b"new"), &roots),
            Response::Ok
        ));
        let mode = fs::metadata(granted.join("shared.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_directories_refuse_writes() {
        let dir = temp("ro");
        let granted = dir.join("granted");
        fs::write(granted.join("kept.txt"), b"original").unwrap();
        let roots = grant(&granted, false);
        assert!(denied(&handle_request(
            write_req(&granted.join("kept.txt"), b"x"),
            &roots
        )));
        assert_eq!(fs::read(granted.join("kept.txt")).unwrap(), b"original");
        assert!(matches!(
            handle_request(read_req(&granted.join("kept.txt")), &roots),
            Response::OkData { .. }
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_create_no_directories_and_leave_no_temporary_files() {
        let dir = temp("nodirs");
        let granted = dir.join("granted");
        let roots = grant(&granted, true);
        assert!(denied(&handle_request(
            write_req(&granted.join("missing/f"), b"x"),
            &roots
        )));
        assert!(!granted.join("missing").exists());
        assert!(denied(&handle_request(write_req(&granted, b"x"), &roots)));
        let left: Vec<_> = fs::read_dir(&granted).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_files_are_refused() {
        let dir = temp("size");
        let granted = dir.join("granted");
        fs::File::create(granted.join("big"))
            .unwrap()
            .set_len(MAX_FILE as u64 + 1)
            .unwrap();
        let roots = grant(&granted, true);
        assert!(denied(&handle_request(
            read_req(&granted.join("big")),
            &roots
        )));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlong_requests_end_the_connection() {
        let dir = temp("line");
        let granted = dir.join("granted");
        let roots = grant(&granted, true);
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let serve = std::thread::spawn(move || handle_connection(server, &roots));
        let mut writer = client.try_clone().unwrap();
        let sent = std::thread::spawn(move || {
            let _ = writer.write_all(&vec![b'x'; MAX_LINE + 64]);
        });
        let mut reply = String::new();
        BufReader::new(client).read_line(&mut reply).unwrap();
        serve.join().unwrap();
        let _ = sent.join();
        assert!(reply.contains("request too long"), "{reply}");
        let _ = fs::remove_dir_all(&dir);
    }
}
