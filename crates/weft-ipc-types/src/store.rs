//! The layout of a package store and the revisions it holds.
//!
//! Each installed revision of a package is an immutable directory named by
//! its content digest, `<store>/.revisions/<app_id>/<digest>`. The active
//! revision is chosen by `<store>/<app_id>`, a symbolic link to it that one
//! rename replaces, so an update or rollback switches every new launch at
//! once and an interrupted one leaves the previous choice in place. Stores
//! written before revisions existed hold the package directly in
//! `<store>/<app_id>`; that layout is still read.
//!
//! A session pins the revision it was launched from with a shared lock on
//! the revision directory, held for the session's lifetime. Package
//! operations remove a revision only after taking that lock exclusively, so
//! a live session keeps running on the bytes it started with.

use std::fs::File;
use std::path::{Path, PathBuf};

/// The directory of a store that holds package revisions.
pub const REVISIONS_DIR: &str = ".revisions";

/// The directory holding the revisions of `app_id`.
pub fn revisions_of(store_root: &Path, app_id: &str) -> PathBuf {
    store_root.join(REVISIONS_DIR).join(app_id)
}

/// The link target that makes `revision` the active revision of `app_id`,
/// relative to the store root.
pub fn link_target(app_id: &str, revision: &str) -> PathBuf {
    Path::new(REVISIONS_DIR).join(app_id).join(revision)
}

/// Whether `name` is a revision name: the hex content digest of a package.
pub fn is_revision_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Where the active package of `app_id` is in a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Active {
    /// A revision, by name, in `.revisions/<app_id>/`.
    Revision { name: String, dir: PathBuf },
    /// A package directory installed before revisions existed.
    Directory(PathBuf),
}

impl Active {
    /// The package root.
    pub fn dir(&self) -> &Path {
        match self {
            Self::Revision { dir, .. } | Self::Directory(dir) => dir,
        }
    }
}

/// Why the active package of a store cannot be determined.
#[derive(Debug)]
pub enum StoreError {
    Io(PathBuf, std::io::Error),
    /// `<store>/<app_id>` is neither a package directory nor a link to one
    /// of its revisions.
    NotAPackage(PathBuf),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "cannot read {}: {e}", path.display()),
            Self::NotAPackage(path) => write!(
                f,
                "{} is neither a package directory nor a link to one of its revisions",
                path.display()
            ),
        }
    }
}

impl std::error::Error for StoreError {}

/// The active package of `app_id` in `store_root`, if any. A link is
/// followed only when it names one of the app's own revisions, exactly as
/// package operations write it, and that revision is a directory.
pub fn active(store_root: &Path, app_id: &str) -> Result<Option<Active>, StoreError> {
    let entry = store_root.join(app_id);
    let metadata = match std::fs::symlink_metadata(&entry) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StoreError::Io(entry, e)),
    };
    if metadata.is_dir() {
        return Ok(Some(Active::Directory(entry)));
    }
    if !metadata.file_type().is_symlink() {
        return Err(StoreError::NotAPackage(entry));
    }
    let target = std::fs::read_link(&entry).map_err(|e| StoreError::Io(entry.clone(), e))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| is_revision_name(n))
        .map(str::to_owned);
    let Some(name) = name.filter(|n| target == link_target(app_id, n)) else {
        return Err(StoreError::NotAPackage(entry));
    };
    let dir = revisions_of(store_root, app_id).join(&name);
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => Ok(Some(Active::Revision { name, dir })),
        Ok(_) => Err(StoreError::NotAPackage(entry)),
        Err(e) => Err(StoreError::Io(dir, e)),
    }
}

/// A shared lock on a package directory, held while a session runs from it.
#[derive(Debug)]
pub struct Pin {
    _dir: File,
}

/// Pins the package directory `dir`. The lock is taken on the directory
/// that is opened, and the path must still name that directory afterwards:
/// a revision removed or replaced in between is not pinned. A directory a
/// package operation holds is not waited for, so nobody able to lock a
/// package directory can stall launches; the launch is refused instead.
pub fn pin(dir: &Path) -> std::io::Result<Pin> {
    let file = File::open(dir)?;
    match file.try_lock_shared() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(std::io::Error::other(format!(
                "{} is being changed; try again",
                dir.display()
            )));
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(e),
    }
    if !same_file(&file, dir)? {
        return Err(std::io::Error::other(format!(
            "{} changed while it was being pinned",
            dir.display()
        )));
    }
    Ok(Pin { _dir: file })
}

/// Takes the exclusive lock on a package directory that no session pins,
/// or returns `None` while one does.
pub fn try_claim(dir: &Path) -> std::io::Result<Option<File>> {
    let file = File::open(dir)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(unix)]
fn same_file(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let opened = file.metadata()?;
    let named = std::fs::symlink_metadata(path)?;
    Ok(opened.dev() == named.dev() && opened.ino() == named.ino())
}

#[cfg(not(unix))]
fn same_file(_file: &File, _path: &Path) -> std::io::Result<bool> {
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weft_store_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const REV: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[cfg(unix)]
    #[test]
    fn only_links_to_the_apps_own_revisions_are_followed() {
        use std::os::unix::fs::symlink;
        let root = store("links");
        std::fs::create_dir_all(revisions_of(&root, "org.a").join(REV)).unwrap();
        std::fs::create_dir_all(revisions_of(&root, "org.b").join(REV)).unwrap();
        assert!(active(&root, "org.a").unwrap().is_none());

        symlink(link_target("org.a", REV), root.join("org.a")).unwrap();
        assert_eq!(
            active(&root, "org.a").unwrap(),
            Some(Active::Revision {
                name: REV.to_owned(),
                dir: revisions_of(&root, "org.a").join(REV),
            })
        );
        // Another app's revision, an absolute target, a non-revision name.
        for target in [
            link_target("org.b", REV),
            revisions_of(&root, "org.c").join(REV),
            PathBuf::from(".revisions/org.c/latest"),
        ] {
            let _ = std::fs::remove_file(root.join("org.c"));
            std::fs::create_dir_all(revisions_of(&root, "org.c").join(REV)).unwrap();
            symlink(&target, root.join("org.c")).unwrap();
            assert!(
                matches!(active(&root, "org.c"), Err(StoreError::NotAPackage(_))),
                "{}",
                target.display()
            );
        }
        // A package installed before revisions existed.
        std::fs::create_dir(root.join("org.d")).unwrap();
        assert_eq!(
            active(&root, "org.d").unwrap(),
            Some(Active::Directory(root.join("org.d")))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_pinned_directory_cannot_be_claimed() {
        let root = store("pin");
        let dir = root.join("rev");
        std::fs::create_dir(&dir).unwrap();
        let pinned = pin(&dir).unwrap();
        assert!(try_claim(&dir).unwrap().is_none());
        // Other sessions can pin it too.
        let again = pin(&dir).unwrap();
        drop((pinned, again));
        let claimed = try_claim(&dir).unwrap();
        assert!(claimed.is_some());
        drop(claimed);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
