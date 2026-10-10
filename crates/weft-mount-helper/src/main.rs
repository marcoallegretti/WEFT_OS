//! Mounts a dm-verity protected EROFS package image for weft-appd, and
//! unmounts it.
//!
//! The helper is installed setuid root, and every argument and the whole
//! environment come from the user who runs it. It therefore:
//!
//! - mounts only at an empty directory `/run/user/<uid>/weft/mnt/<name>` of
//!   the real user, where each component from `<uid>` on is owned by that
//!   user, not writable by others and opened without following symbolic
//!   links; the mount goes through the opened directory, so the path cannot
//!   be swapped meanwhile;
//! - opens the image and its hash tree with the real user's permissions;
//! - mounts read-only, without set-user-ID programs or device files;
//! - unmounts only an EROFS filesystem at such a directory;
//! - runs `veritysetup` from the absolute path fixed when it was built
//!   (`WEFT_VERITYSETUP`), with an empty environment, and the mount
//!   system calls itself.

use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path};

use anyhow::{Context, bail};

const VERITYSETUP: &str = match option_env!("WEFT_VERITYSETUP") {
    Some(path) => path,
    None => "/usr/sbin/veritysetup",
};
/// The parent of every user's runtime directory.
const RUN_USER: &str = "/run/user";
const EROFS_SUPER_MAGIC: libc::c_long = 0xE0F5_E1E2;
/// How many package images one user may have open at a time, so a user
/// cannot use up the system's device-mapper and loop devices.
const MAX_DEVICES_PER_USER: usize = 64;

fn main() {
    // Panics print one line too, whatever RUST_BACKTRACE says.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("weft-mount-helper: internal error: {info}");
    }));
    // A caller can start the helper with standard input, output or error
    // closed; a file opened later would then take that number and be
    // misdirected. They are opened on /dev/null first.
    for fd in 0..=2 {
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0
            && unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR) } != fd
        {
            std::process::abort();
        }
    }
    // Errors are printed as messages only: a caller's RUST_BACKTRACE has no
    // effect.
    if let Err(e) = run() {
        eprintln!("weft-mount-helper: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match (args.get(1).map(String::as_str), args.len()) {
        (Some("mount"), 6) => {
            require_root()?;
            cmd_mount(
                Path::new(&args[2]),
                Path::new(&args[3]),
                &args[4],
                Path::new(&args[5]),
            )
        }
        (Some("umount"), 3) => {
            require_root()?;
            cmd_umount(Path::new(&args[2]))
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  weft-mount-helper mount <img> <hash_dev> <root_hash> <mountpoint>");
            eprintln!("  weft-mount-helper umount <mountpoint>");
            std::process::exit(1);
        }
    }
}

fn require_root() -> anyhow::Result<()> {
    let euid = unsafe { libc::geteuid() };
    if euid != 0 {
        bail!("weft-mount-helper must run as root (euid={euid})");
    }
    Ok(())
}

fn real_ids() -> (libc::uid_t, libc::gid_t) {
    unsafe { (libc::getuid(), libc::getgid()) }
}

/// A mountpoint that passed the checks, held open.
struct Mountpoint {
    dir: OwnedFd,
    uid: libc::uid_t,
    name: String,
}

impl Mountpoint {
    /// The directory as a path the kernel resolves to the open directory.
    fn proc_path(&self) -> String {
        format!("/proc/self/fd/{}", self.dir.as_raw_fd())
    }

    /// The device-mapper name, from the user and the mountpoint's name,
    /// which weft-appd chooses at random, so concurrent sessions get
    /// distinct devices and no user can name another user's.
    fn device_name(&self) -> String {
        format!("{}{}", device_prefix(self.uid), self.name)
    }
}

fn device_prefix(uid: libc::uid_t) -> String {
    format!("weft-{uid}-")
}

/// The names of the device-mapper devices, from the kernel's own list.
fn device_names() -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir("/sys/block").context("cannot list block devices")? {
        let path = entry
            .context("cannot list block devices")?
            .path()
            .join("dm/name");
        match std::fs::read_to_string(&path) {
            Ok(name) => names.push(name.trim_end().to_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }
    Ok(names)
}

/// Serializes device creation, so the per-user limit holds against
/// concurrent mounts. The lock lives in a directory only root can write.
fn lock_devices() -> anyhow::Result<File> {
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/run/weft-mount-helper.lock")
        .context("cannot open the device lock")?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        bail!(
            "cannot lock the devices: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(lock)
}

/// A mountpoint name: what weft-appd's random tokens are made of.
fn is_mountpoint_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A dm-verity root hash in hex, as `veritysetup format` prints it.
fn is_root_hash(text: &str) -> bool {
    (64..=128).contains(&text.len())
        && text.len().is_multiple_of(2)
        && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn openat_dir(parent: Option<&OwnedFd>, name: &Path) -> std::io::Result<OwnedFd> {
    let name = CString::new(name.as_os_str().as_encoded_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let dirfd = parent.map_or(libc::AT_FDCWD, |p| p.as_raw_fd());
    let fd = unsafe {
        libc::openat(
            dirfd,
            name.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn fstat(fd: &OwnedFd) -> std::io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(st)
}

/// Opens `mountpoint`, which must be `<run_user>/<uid>/weft/mnt/<name>`.
/// Every directory from `<uid>` on must belong to `uid` and not be writable
/// by others; for a mount, the mountpoint itself too, and it must be empty.
fn open_mountpoint(
    run_user: &Path,
    uid: libc::uid_t,
    mountpoint: &Path,
    for_mount: bool,
) -> anyhow::Result<Mountpoint> {
    let base = run_user.join(uid.to_string()).join("weft/mnt");
    let name = mountpoint
        .strip_prefix(&base)
        .ok()
        .filter(|rest| {
            let mut components = rest.components();
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
        })
        .and_then(|rest| rest.to_str())
        .filter(|name| is_mountpoint_name(name))
        .with_context(|| {
            format!(
                "{} is not a mountpoint in {}",
                mountpoint.display(),
                base.display()
            )
        })?
        .to_owned();

    let mut dir = openat_dir(None, run_user)
        .with_context(|| format!("cannot open {}", run_user.display()))?;
    let uid_dir = uid.to_string();
    let components = [uid_dir.as_str(), "weft", "mnt", name.as_str()];
    for (i, component) in components.iter().enumerate() {
        dir = openat_dir(Some(&dir), Path::new(component))
            .with_context(|| format!("cannot open {component} in the path to the mountpoint"))?;
        // For an unmount the last one is the mounted filesystem's root.
        if i == components.len() - 1 && !for_mount {
            break;
        }
        let st = fstat(&dir)?;
        if st.st_uid != uid || st.st_mode & 0o022 != 0 {
            bail!("{component} in the path to the mountpoint is not the user's own directory");
        }
    }
    let mountpoint = Mountpoint { dir, uid, name };
    if for_mount
        && std::fs::read_dir(mountpoint.proc_path())
            .context("cannot read the mountpoint")?
            .next()
            .is_some()
    {
        bail!("the mountpoint is not empty");
    }
    Ok(mountpoint)
}

/// Opens `path` with the real user's permissions, as a regular file.
fn open_as_user(path: &Path, uid: libc::uid_t, gid: libc::gid_t) -> anyhow::Result<File> {
    let euid = unsafe { libc::geteuid() };
    let egid = unsafe { libc::getegid() };
    if unsafe { libc::setegid(gid) } != 0 || unsafe { libc::seteuid(uid) } != 0 {
        let error = std::io::Error::last_os_error();
        restore_ids(euid, egid);
        bail!("cannot take the user's permissions: {error}");
    }
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path);
    restore_ids(euid, egid);
    let file = opened.with_context(|| format!("cannot open {}", path.display()))?;
    if !file.metadata()?.file_type().is_file() {
        bail!("{} is not a regular file", path.display());
    }
    Ok(file)
}

/// Returns to the helper's own effective IDs; continuing without them is
/// not safe, so failing aborts.
fn restore_ids(euid: libc::uid_t, egid: libc::gid_t) {
    if unsafe { libc::seteuid(euid) } != 0 || unsafe { libc::setegid(egid) } != 0 {
        std::process::abort();
    }
}

/// Lets a child process inherit `file`.
fn inheritable(file: &File) -> anyhow::Result<String> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        bail!("cannot pass a file on: {}", std::io::Error::last_os_error());
    }
    Ok(format!("/proc/self/fd/{fd}"))
}

fn veritysetup(args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new(VERITYSETUP)
        .env_clear()
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .with_context(|| format!("cannot run {VERITYSETUP}"))?;
    if !status.success() {
        bail!("veritysetup {} failed with {status}", args[0]);
    }
    Ok(())
}

fn cmd_mount(
    img: &Path,
    hash_dev: &Path,
    root_hash: &str,
    mountpoint: &Path,
) -> anyhow::Result<()> {
    if !is_root_hash(root_hash) {
        bail!("the root hash is not a hex digest");
    }
    let (uid, gid) = real_ids();
    let target = open_mountpoint(Path::new(RUN_USER), uid, mountpoint, true)?;
    let image = open_as_user(img, uid, gid)?;
    let hash_tree = open_as_user(hash_dev, uid, gid)?;
    let dev_name = target.device_name();
    let _lock = lock_devices()?;
    let names = device_names()?;
    // A device left by a mount interrupted before it was mounted here; the
    // empty mountpoint shows it is not in use.
    if names.contains(&dev_name) {
        veritysetup(&["close", &dev_name])?;
    }
    let prefix = device_prefix(uid);
    let open = names
        .iter()
        .filter(|name| name.starts_with(&prefix) && **name != dev_name)
        .count();
    if open >= MAX_DEVICES_PER_USER {
        bail!("too many package images are open ({MAX_DEVICES_PER_USER})");
    }

    veritysetup(&[
        "open",
        &inheritable(&image)?,
        &dev_name,
        &inheritable(&hash_tree)?,
        root_hash,
    ])?;

    let source = CString::new(format!("/dev/mapper/{dev_name}"))?;
    let path = CString::new(target.proc_path())?;
    let fstype = CString::new("erofs")?;
    // Packages hold components and UI files, which are read, never run.
    let flags = libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    let mounted = unsafe {
        libc::mount(
            source.as_ptr(),
            path.as_ptr(),
            fstype.as_ptr(),
            flags,
            std::ptr::null(),
        )
    };
    if mounted != 0 {
        let error = std::io::Error::last_os_error();
        let _ = veritysetup(&["close", &dev_name]);
        bail!("mount failed: {error}");
    }
    eprintln!("mounted: {} -> {}", img.display(), mountpoint.display());
    Ok(())
}

fn cmd_umount(mountpoint: &Path) -> anyhow::Result<()> {
    let (uid, _) = real_ids();
    let target = open_mountpoint(Path::new(RUN_USER), uid, mountpoint, false)?;
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(target.dir.as_raw_fd(), &mut fs) } != 0 {
        bail!(
            "cannot inspect the mountpoint: {}",
            std::io::Error::last_os_error()
        );
    }
    if fs.f_type as libc::c_long != EROFS_SUPER_MAGIC {
        // A mount interrupted after its device opened leaves the device and
        // an empty mountpoint; the device is the caller's own.
        let dev_name = target.device_name();
        if device_names()?.contains(&dev_name) {
            veritysetup(&["close", &dev_name])?;
            eprintln!("closed the unmounted device for {}", mountpoint.display());
            return Ok(());
        }
        bail!("no package image is mounted at {}", mountpoint.display());
    }
    let path = CString::new(target.proc_path())?;
    if unsafe { libc::umount2(path.as_ptr(), 0) } != 0 {
        bail!("umount failed: {}", std::io::Error::last_os_error());
    }
    veritysetup(&["close", &target.device_name()])?;
    eprintln!("unmounted: {}", mountpoint.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Layout {
        root: std::path::PathBuf,
        uid: libc::uid_t,
    }

    impl Layout {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("weft_mount_helper_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let uid = unsafe { libc::getuid() };
            let mnt = root.join(uid.to_string()).join("weft/mnt");
            std::fs::create_dir_all(&mnt).unwrap();
            for dir in [
                root.join(uid.to_string()),
                root.join(format!("{uid}/weft")),
                mnt,
            ] {
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            Layout { root, uid }
        }

        fn mnt(&self) -> std::path::PathBuf {
            self.root.join(self.uid.to_string()).join("weft/mnt")
        }
    }

    impl Drop for Layout {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn an_empty_directory_of_the_user_is_a_mountpoint() {
        let layout = Layout::new("ok");
        let point = layout.mnt().join("0123abcd");
        std::fs::create_dir(&point).unwrap();
        let target = open_mountpoint(&layout.root, layout.uid, &point, true).unwrap();
        assert_eq!(
            target.device_name(),
            format!("weft-{}-0123abcd", layout.uid)
        );
        // The open directory is the mountpoint.
        assert_eq!(
            std::fs::canonicalize(target.proc_path()).unwrap(),
            std::fs::canonicalize(&point).unwrap()
        );
    }

    #[test]
    fn mountpoints_elsewhere_are_refused() {
        let layout = Layout::new("elsewhere");
        let mnt = layout.mnt();
        std::fs::create_dir(mnt.join("ok")).unwrap();
        for path in [
            std::path::PathBuf::from("/etc"),
            layout.root.join("other"),
            mnt.clone(),
            mnt.join("ok/deeper"),
            mnt.join("../mnt/ok"),
            mnt.join("bad.name"),
            mnt.join("x".repeat(65)),
            // Another user's directory.
            layout
                .root
                .join((layout.uid + 1).to_string())
                .join("weft/mnt/ok"),
        ] {
            assert!(
                open_mountpoint(&layout.root, layout.uid, &path, true).is_err(),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn symbolic_links_and_shared_directories_are_refused() {
        let layout = Layout::new("links");
        let mnt = layout.mnt();
        // The mountpoint is a link to a directory elsewhere.
        let elsewhere = layout.root.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, mnt.join("link")).unwrap();
        assert!(open_mountpoint(&layout.root, layout.uid, &mnt.join("link"), true).is_err());
        assert!(open_mountpoint(&layout.root, layout.uid, &mnt.join("link"), false).is_err());

        // A directory others may write to.
        std::fs::create_dir(mnt.join("shared")).unwrap();
        std::fs::set_permissions(mnt.join("shared"), std::fs::Permissions::from_mode(0o777))
            .unwrap();
        assert!(open_mountpoint(&layout.root, layout.uid, &mnt.join("shared"), true).is_err());

        // A link in the path above the mountpoint.
        let weft = layout.root.join(format!("{}/weft", layout.uid));
        std::fs::rename(&weft, layout.root.join("moved")).unwrap();
        std::os::unix::fs::symlink(layout.root.join("moved"), &weft).unwrap();
        std::fs::create_dir(layout.root.join("moved/mnt/ok")).unwrap();
        assert!(open_mountpoint(&layout.root, layout.uid, &mnt.join("ok"), true).is_err());
    }

    #[test]
    fn a_mountpoint_must_be_empty_and_the_users_own() {
        let layout = Layout::new("full");
        let point = layout.mnt().join("full");
        std::fs::create_dir(&point).unwrap();
        std::fs::write(point.join("file"), b"x").unwrap();
        assert!(open_mountpoint(&layout.root, layout.uid, &point, true).is_err());
        // Unmounting looks at the mounted filesystem, not its contents.
        assert!(open_mountpoint(&layout.root, layout.uid, &point, false).is_ok());

        // The same layout seen as another user's.
        let other = layout.uid + 1;
        let theirs = layout.root.join(other.to_string());
        std::fs::rename(layout.root.join(layout.uid.to_string()), &theirs).unwrap();
        std::fs::remove_file(theirs.join("weft/mnt/full/file")).unwrap();
        assert!(open_mountpoint(&layout.root, other, &theirs.join("weft/mnt/full"), true).is_err());
        std::fs::rename(&theirs, layout.root.join(layout.uid.to_string())).unwrap();
    }

    #[test]
    fn root_hashes_are_hex_digests() {
        assert!(is_root_hash(&"a".repeat(64)));
        assert!(is_root_hash(&"0F".repeat(64)));
        for bad in ["", "-a", &"g".repeat(64), &"a".repeat(65), &"a".repeat(130)] {
            assert!(!is_root_hash(bad), "{bad:?}");
        }
    }
}
