//! The package manifest, `wapp.toml`, and the package files it names.
//!
//! This is the one parser for manifests: the packaging tool validates
//! packages with it and the session supervisor resolves launches with it.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// The manifest's file name at the root of a package.
pub const MANIFEST_FILE: &str = "wapp.toml";

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub package: PackageMeta,
    pub runtime: RuntimeMeta,
    pub ui: UiMeta,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PackageMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub capabilities: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeMeta {
    /// The Wasm component, relative to the package root.
    pub module: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UiMeta {
    /// The UI document, relative to the package root.
    pub entry: String,
    /// What happens when the user asks the application's window to close.
    #[serde(default)]
    pub close: ClosePolicy,
}

/// An application's declared unsaved-change policy: what a user close does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClosePolicy {
    /// The window closes at once; the application keeps no unsaved state.
    #[default]
    Immediate,
    /// The page is asked first and may cancel the close, for example to
    /// offer saving unsaved changes; it then closes itself when done.
    Ask,
}

impl ClosePolicy {
    /// The value weft-appd passes to the app shell in `WEFT_APP_CLOSE`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Ask => "ask",
        }
    }
}

/// Why a manifest could not be read.
#[derive(Debug)]
pub enum ManifestError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "cannot read {}: {e}", path.display()),
            Self::Parse(path, e) => write!(f, "invalid {}: {e}", path.display()),
        }
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    /// Reads `<package_root>/wapp.toml`.
    pub fn read(package_root: &Path) -> Result<Self, ManifestError> {
        let path = package_root.join(MANIFEST_FILE);
        let text =
            std::fs::read_to_string(&path).map_err(|e| ManifestError::Io(path.clone(), e))?;
        toml::from_str(&text).map_err(|e| ManifestError::Parse(path, e))
    }

    pub fn capabilities(&self) -> &[String] {
        self.package.capabilities.as_deref().unwrap_or_default()
    }
}

/// Why a manifest entry does not name a usable package file.
#[derive(Debug, PartialEq, Eq)]
pub enum EntryError {
    /// The entry is empty, absolute, or has `.` or `..` components.
    NotContained(String),
    /// A component of the entry is a symbolic link.
    Link(String),
    /// The entry does not exist, or is not a regular file.
    NotAFile(String),
}

impl fmt::Display for EntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotContained(entry) => {
                write!(f, "'{entry}' is not a relative path inside the package")
            }
            Self::Link(entry) => write!(f, "'{entry}' passes through a symbolic link"),
            Self::NotAFile(entry) => write!(f, "'{entry}' is not a file in the package"),
        }
    }
}

impl std::error::Error for EntryError {}

/// The package file a manifest entry names: `entry` must be a relative path
/// of plain components, none of them a symbolic link, ending at a regular
/// file inside `package_root`.
pub fn entry_path(package_root: &Path, entry: &str) -> Result<PathBuf, EntryError> {
    let relative = Path::new(entry);
    let mut components = relative.components().peekable();
    if components.peek().is_none()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(EntryError::NotContained(entry.to_owned()));
    }
    let mut path = package_root.to_path_buf();
    while let Some(component) = components.next() {
        path.push(component);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| EntryError::NotAFile(entry.to_owned()))?;
        if metadata.file_type().is_symlink() {
            return Err(EntryError::Link(entry.to_owned()));
        }
        let last = components.peek().is_none();
        if (last && !metadata.is_file()) || (!last && !metadata.is_dir()) {
            return Err(EntryError::NotAFile(entry.to_owned()));
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weft_manifest_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join("app.wasm"), b"\0asm").unwrap();
        std::fs::write(dir.join("ui/index.html"), b"<p>").unwrap();
        dir
    }

    #[test]
    fn entries_resolve_to_package_files() {
        let dir = package("files");
        assert_eq!(entry_path(&dir, "app.wasm").unwrap(), dir.join("app.wasm"));
        assert_eq!(
            entry_path(&dir, "ui/index.html").unwrap(),
            dir.join("ui/index.html")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entries_must_stay_inside_the_package() {
        let dir = package("contained");
        for entry in [
            "",
            "/etc/passwd",
            "../app.wasm",
            "ui/../app.wasm",
            "./app.wasm",
        ] {
            assert_eq!(
                entry_path(&dir, entry),
                Err(EntryError::NotContained(entry.to_owned())),
                "{entry:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entries_must_name_existing_regular_files() {
        let dir = package("kinds");
        for entry in ["missing.wasm", "ui", "app.wasm/x"] {
            assert_eq!(
                entry_path(&dir, entry),
                Err(EntryError::NotAFile(entry.to_owned())),
                "{entry:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn entries_never_follow_links() {
        let dir = package("links");
        std::os::unix::fs::symlink(dir.join("app.wasm"), dir.join("linked.wasm")).unwrap();
        std::os::unix::fs::symlink(dir.join("ui"), dir.join("view")).unwrap();
        assert_eq!(
            entry_path(&dir, "linked.wasm"),
            Err(EntryError::Link("linked.wasm".to_owned()))
        );
        assert_eq!(
            entry_path(&dir, "view/index.html"),
            Err(EntryError::Link("view/index.html".to_owned()))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn manifests_parse_with_their_entries() {
        let dir = package("parse");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            "[package]\nid = \"org.weft.test\"\nname = \"T\"\nversion = \"1.0.0\"\n\
             capabilities = [\"sys:notifications\"]\n\
             [runtime]\nmodule = \"app.wasm\"\n[ui]\nentry = \"ui/index.html\"\n",
        )
        .unwrap();
        let manifest = Manifest::read(&dir).unwrap();
        assert_eq!(manifest.package.id, "org.weft.test");
        assert_eq!(manifest.capabilities(), ["sys:notifications"]);
        assert_eq!(manifest.ui.close, ClosePolicy::Immediate);
        std::fs::write(
            dir.join(MANIFEST_FILE),
            "[package]\nid = \"org.weft.test\"\nname = \"T\"\nversion = \"1.0.0\"\n\
             [runtime]\nmodule = \"app.wasm\"\n[ui]\nentry = \"ui/index.html\"\nclose = \"ask\"\n",
        )
        .unwrap();
        assert_eq!(Manifest::read(&dir).unwrap().ui.close, ClosePolicy::Ask);
        std::fs::write(
            dir.join(MANIFEST_FILE),
            "[package]\nid = \"org.weft.test\"\nname = \"T\"\nversion = \"1.0.0\"\n\
             [runtime]\nmodule = \"app.wasm\"\n[ui]\nentry = \"ui/index.html\"\nclose = \"later\"\n",
        )
        .unwrap();
        assert!(matches!(
            Manifest::read(&dir),
            Err(ManifestError::Parse(..))
        ));
        std::fs::write(dir.join(MANIFEST_FILE), "[package]\nid = \"x\"\n").unwrap();
        assert!(matches!(
            Manifest::read(&dir),
            Err(ManifestError::Parse(..))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
