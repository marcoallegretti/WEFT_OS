//! Package identity and storage rules shared by the packaging tool and the
//! supervisor.

use std::path::{Path, PathBuf};

/// Whether `id` is a valid application ID: at least three dot-separated
/// segments, each starting with a lowercase ASCII letter and containing only
/// lowercase ASCII letters and digits. A valid ID is safe to use as a single
/// path component.
pub fn is_valid_app_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 3
        && parts.iter().all(|p| {
            p.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// The user's data home: `$XDG_DATA_HOME` when it is an absolute path,
/// otherwise `$HOME/.local/share`.
pub fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|home| home.is_absolute())
                .map(|home| home.join(".local/share"))
        })
}

/// The private data directory of an application, independent of where or
/// whether its package is installed.
pub fn app_data_dir(data_home: &Path, app_id: &str) -> PathBuf {
    data_home.join("weft/app-data").join(app_id)
}

/// Where earlier versions kept application data: inside the user package
/// store, under the installed package directory.
pub fn legacy_app_data_dir(home: &Path, app_id: &str) -> PathBuf {
    home.join(".local/share/weft/apps")
        .join(app_id)
        .join("data")
}

/// The result of moving data from the earlier layout.
#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// No data in the earlier layout.
    NotNeeded,
    /// The directory was moved to the new location.
    Moved,
}

/// Moves `legacy` to `target` if `legacy` is a directory, in one rename so
/// the data is never partly in both places. Refuses, leaving both untouched,
/// when `target` already exists, since either copy may hold the user's
/// latest data, and when `legacy` is a symbolic link.
pub fn migrate_app_data(legacy: &Path, target: &Path) -> std::io::Result<Migration> {
    use std::io::{Error, ErrorKind};
    let metadata = match std::fs::symlink_metadata(legacy) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Migration::NotNeeded),
        Err(e) => return Err(e),
    };
    if !metadata.is_dir() {
        return Err(Error::other(format!(
            "{} is not a directory; move it aside to continue",
            legacy.display()
        )));
    }
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(Error::new(
            ErrorKind::AlreadyExists,
            format!(
                "app data exists both in {} and in {}; keep one of them",
                legacy.display(),
                target.display()
            ),
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(legacy, target)?;
    Ok(Migration::Moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids() {
        assert!(is_valid_app_id("org.weft.demo"));
        assert!(is_valid_app_id("com.example2.app3"));
        for bad in [
            "",
            "org.weft",
            "Org.weft.demo",
            "org..demo",
            "../x.y.z",
            "org.weft.a::b",
            "org.1weft.demo",
        ] {
            assert!(!is_valid_app_id(bad), "{bad:?}");
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weft_migrate_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn data_lives_outside_the_package_store() {
        let home = Path::new("/home/u");
        let data = app_data_dir(&home.join(".local/share"), "org.weft.demo");
        assert_eq!(
            data,
            Path::new("/home/u/.local/share/weft/app-data/org.weft.demo")
        );
        assert!(!data.starts_with("/home/u/.local/share/weft/apps"));
        assert_eq!(
            legacy_app_data_dir(home, "org.weft.demo"),
            Path::new("/home/u/.local/share/weft/apps/org.weft.demo/data")
        );
    }

    #[test]
    fn migration_moves_data_intact() {
        let root = temp("move");
        let legacy = root.join("apps/org.weft.notes/data");
        std::fs::create_dir_all(legacy.join("sub")).unwrap();
        std::fs::write(legacy.join("notes.txt"), "line one\nline \\two").unwrap();
        std::fs::write(legacy.join("sub/x"), [0u8, 255, 10]).unwrap();
        let target = root.join("app-data/org.weft.notes");

        assert_eq!(
            migrate_app_data(&legacy, &target).unwrap(),
            Migration::Moved
        );
        assert!(!legacy.exists());
        assert_eq!(
            std::fs::read_to_string(target.join("notes.txt")).unwrap(),
            "line one\nline \\two"
        );
        assert_eq!(std::fs::read(target.join("sub/x")).unwrap(), [0u8, 255, 10]);
        assert_eq!(
            migrate_app_data(&legacy, &target).unwrap(),
            Migration::NotNeeded
        );
    }

    #[test]
    fn migration_refuses_conflicts_and_leaves_both() {
        let root = temp("conflict");
        let legacy = root.join("old");
        let target = root.join("new");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(legacy.join("a"), "old").unwrap();
        std::fs::write(target.join("a"), "new").unwrap();
        let err = migrate_app_data(&legacy, &target).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(legacy.join("a")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(target.join("a")).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn migration_refuses_a_linked_legacy_directory() {
        let root = temp("link");
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("old")).unwrap();
        assert!(migrate_app_data(&root.join("old"), &root.join("new")).is_err());
        assert!(!root.join("new").exists());
    }
}
