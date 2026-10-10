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

/// The files of a verified package image in a package store: the EROFS
/// image, its dm-verity hash tree and the hex root hash, named after the
/// application ID. `weft-pack build-image` and `build-verity` produce these
/// names and the supervisor looks for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFiles {
    pub image: PathBuf,
    pub hash_tree: PathBuf,
    pub root_hash: PathBuf,
}

impl ImageFiles {
    pub fn in_store(store_root: &Path, app_id: &str) -> Self {
        Self::for_image(&store_root.join(format!("{app_id}.app.img")))
    }

    /// The companions of `image`, named by replacing its extension.
    pub fn for_image(image: &Path) -> Self {
        Self {
            image: image.to_path_buf(),
            hash_tree: image.with_extension("hash"),
            root_hash: image.with_extension("roothash"),
        }
    }

    /// Whether any of the three files is present, whether or not it is
    /// usable.
    pub fn any_present(&self) -> bool {
        [&self.image, &self.hash_tree, &self.root_hash]
            .iter()
            .any(|path| std::fs::symlink_metadata(path).is_ok())
    }
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

/// The home directory, when `HOME` is set to an absolute path.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// Where app data for `app_id` already exists, in the current layout or in
/// the earlier one under `home`, if anywhere. Data in the earlier layout is
/// moved to the current one when the app next launches, so it counts too.
pub fn existing_app_data(
    data_home: &Path,
    home: &Path,
    app_id: &str,
) -> std::io::Result<Option<PathBuf>> {
    let places = [
        app_data_dir(data_home, app_id),
        legacy_app_data_dir(home, app_id),
    ];
    for place in places {
        match std::fs::symlink_metadata(&place) {
            Ok(_) => return Ok(Some(place)),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// The result of moving data from the earlier layout.
#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// No data in the earlier layout.
    NotNeeded,
    /// The directory was moved to the new location.
    Moved,
}

/// Why data in the earlier layout could not be moved. Nothing was changed.
#[derive(Debug)]
pub enum MigrationError {
    /// Data exists in both locations; either may hold the user's latest
    /// data, so the user has to choose.
    Conflict {
        legacy: PathBuf,
        target: PathBuf,
    },
    /// The two locations are on different filesystems, so the data cannot
    /// be moved in one step.
    CrossDevice {
        legacy: PathBuf,
        target: PathBuf,
    },
    /// The earlier location is not a plain directory.
    NotADirectory(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { legacy, target } => write!(
                f,
                "app data exists both in {} and in {}; keep one of them and remove the other",
                legacy.display(),
                target.display()
            ),
            Self::CrossDevice { legacy, target } => write!(
                f,
                "app data in {} cannot be moved to {} on another filesystem; move it there by hand",
                legacy.display(),
                target.display()
            ),
            Self::NotADirectory(path) => write!(
                f,
                "{} is not a plain directory; move it aside to continue",
                path.display()
            ),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for MigrationError {}

/// Moves `legacy` to `target` if `legacy` is a directory, in one rename so
/// the data is never partly in both places, and then removes the earlier
/// package directory if that leaves it empty. Refuses, changing nothing,
/// when `target` already exists (even empty), when `legacy` is a symbolic
/// link or not a directory, and when the rename would cross filesystems.
pub fn migrate_app_data(legacy: &Path, target: &Path) -> Result<Migration, MigrationError> {
    let conflict = || MigrationError::Conflict {
        legacy: legacy.to_path_buf(),
        target: target.to_path_buf(),
    };
    let metadata = match std::fs::symlink_metadata(legacy) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Migration::NotNeeded),
        Err(e) => return Err(MigrationError::Io(e)),
    };
    if !metadata.is_dir() {
        return Err(MigrationError::NotADirectory(legacy.to_path_buf()));
    }
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(conflict());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(MigrationError::Io)?;
    }
    match rename_no_replace(legacy, target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(conflict()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            return Err(MigrationError::CrossDevice {
                legacy: legacy.to_path_buf(),
                target: target.to_path_buf(),
            });
        }
        Err(e) => return Err(MigrationError::Io(e)),
    }
    if let Some(package_dir) = legacy.parent() {
        // Fails, harmlessly, unless the directory is now empty.
        let _ = std::fs::remove_dir(package_dir);
    }
    Ok(Migration::Moved)
}

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing
/// an existing `to`, even one created after the caller checked for it.
#[cfg(target_os = "linux")]
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(std::io::Error::from)
}

#[cfg(not(target_os = "linux"))]
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

/// Makes `dir` accessible only to its owner. A symbolic link is refused
/// rather than followed.
pub fn make_private(dir: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a plain directory",
            dir.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_companions_share_the_image_name() {
        let files = ImageFiles::in_store(Path::new("/store"), "org.weft.demo.notes");
        assert_eq!(files.image, Path::new("/store/org.weft.demo.notes.app.img"));
        assert_eq!(
            files.hash_tree,
            Path::new("/store/org.weft.demo.notes.app.hash")
        );
        assert_eq!(
            files.root_hash,
            Path::new("/store/org.weft.demo.notes.app.roothash")
        );
        assert_eq!(ImageFiles::for_image(&files.image), files);
    }

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
    fn migration_keeps_a_package_directory_with_other_content() {
        let root = temp("keep-package");
        let legacy = root.join("apps/org.weft.app/data");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(root.join("apps/org.weft.app/wapp.toml"), "").unwrap();
        migrate_app_data(&legacy, &root.join("new")).unwrap();
        assert!(root.join("apps/org.weft.app/wapp.toml").exists());
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
        assert!(matches!(err, MigrationError::Conflict { .. }));
        // An empty target is a conflict too: it may be in use.
        std::fs::remove_file(target.join("a")).unwrap();
        assert!(matches!(
            migrate_app_data(&legacy, &target),
            Err(MigrationError::Conflict { .. })
        ));
        std::fs::write(target.join("a"), "new").unwrap();
        assert_eq!(std::fs::read_to_string(legacy.join("a")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(target.join("a")).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn migration_refuses_a_linked_legacy_directory() {
        let root = temp("link");
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("old")).unwrap();
        assert!(matches!(
            migrate_app_data(&root.join("old"), &root.join("new")),
            Err(MigrationError::NotADirectory(_))
        ));
        assert!(!root.join("new").exists());
    }
}
