//! The package a session runs, resolved once before anything is started.
//!
//! The runtime and the app shell receive the exact files resolved here and
//! never look for the package themselves, so the component, the UI and the
//! capabilities a session is granted all come from the same package root.

use std::path::{Path, PathBuf};

use weft_ipc_types::manifest::{MANIFEST_FILE, Manifest, ManifestError, entry_path};
use weft_ipc_types::package::{ImageFiles, is_valid_app_id};

use crate::grants::Refusal;
use crate::mount::Mount;

pub(crate) struct LaunchPackage {
    pub(crate) app_id: String,
    pub(crate) version: String,
    pub(crate) root: PathBuf,
    /// The Wasm component, from `[runtime].module`.
    pub(crate) module: PathBuf,
    /// The UI document, from `[ui].entry`.
    pub(crate) ui_entry: PathBuf,
    pub(crate) capabilities: Vec<String>,
    /// The image the package root is mounted from, held for as long as the
    /// session runs.
    pub(crate) image: Option<Mount>,
}

/// Resolves `app_id` in the package stores.
pub(crate) fn resolve(app_id: &str) -> Result<LaunchPackage, Refusal> {
    resolve_in(app_id, &crate::app_store_roots())
}

/// Resolves `app_id` in `stores`. A verified image in any store takes
/// precedence over a directory install in any store, so a writable user
/// directory cannot shadow a system image. When any image file is present
/// for the ID, the image must mount: a missing or damaged image never falls
/// back to a directory, which nothing verifies.
fn resolve_in(app_id: &str, stores: &[PathBuf]) -> Result<LaunchPackage, Refusal> {
    use crate::mount::MountError;
    if !is_valid_app_id(app_id) {
        return Err(Refusal::new(400, "invalid app ID"));
    }
    if let Some(files) = stores
        .iter()
        .map(|store| ImageFiles::in_store(store, app_id))
        .find(ImageFiles::any_present)
    {
        let image = crate::mount::mount(app_id, &files).map_err(|e| match e {
            MountError::Invalid(e) => {
                Refusal::new(403, format!("the verified image is invalid: {e}"))
            }
            MountError::Host(e) => {
                Refusal::new(500, format!("the verified image cannot be mounted: {e}"))
            }
        })?;
        let root = image.root().to_path_buf();
        return from_root(app_id, root, Some(image));
    }
    for store in stores {
        let dir = store.join(app_id);
        if std::fs::symlink_metadata(dir.join(MANIFEST_FILE)).is_ok() {
            return from_root(app_id, dir, None);
        }
    }
    Err(Refusal::new(
        404,
        format!("package {app_id} is not installed"),
    ))
}

fn from_root(app_id: &str, root: PathBuf, image: Option<Mount>) -> Result<LaunchPackage, Refusal> {
    // The entries are checked inside `root`, so the root and its manifest
    // must be the directory and file they appear to be, not links to
    // something else.
    let is_dir = std::fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_dir());
    let manifest_is_file =
        std::fs::symlink_metadata(root.join(MANIFEST_FILE)).is_ok_and(|m| m.file_type().is_file());
    if !is_dir || !manifest_is_file {
        return Err(Refusal::new(
            403,
            format!(
                "{} must be a directory holding a regular {MANIFEST_FILE}, not a link",
                root.display()
            ),
        ));
    }
    let manifest = Manifest::read(&root).map_err(|e| match e {
        ManifestError::Io(..) => Refusal::new(500, e.to_string()),
        ManifestError::Parse(..) => Refusal::new(403, e.to_string()),
    })?;
    if manifest.package.id != app_id {
        return Err(Refusal::new(
            403,
            format!(
                "the package in {} declares ID '{}'",
                root.display(),
                manifest.package.id
            ),
        ));
    }
    let entry = |field: &str, value: &str| {
        entry_path(&root, value).map_err(|e| Refusal::new(403, format!("{field}: {e}")))
    };
    let module = entry("runtime.module", &manifest.runtime.module)?;
    let ui_entry = entry("ui.entry", &manifest.ui.entry)?;
    Ok(LaunchPackage {
        app_id: app_id.to_owned(),
        version: manifest.package.version.clone(),
        capabilities: manifest.capabilities().to_vec(),
        module,
        ui_entry,
        root,
        image,
    })
}

impl LaunchPackage {
    /// A package for supervisor tests whose children ignore their files.
    #[cfg(test)]
    pub(crate) fn unresolved(app_id: &str) -> Self {
        Self {
            app_id: app_id.to_owned(),
            version: "0.0.0".to_owned(),
            root: PathBuf::from("/nonexistent"),
            module: PathBuf::from("/nonexistent/app.wasm"),
            ui_entry: PathBuf::from("/nonexistent/ui/index.html"),
            capabilities: Vec::new(),
            image: None,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "org.weft.test.resolve";

    /// A store holding a directory install of `ID`, with WEFT_APP_STORE
    /// pointing at it. Callers hold env_lock.
    fn store(name: &str, manifest_id: &str, module: &str) -> PathBuf {
        let store =
            std::env::temp_dir().join(format!("weft_appd_launch_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&store);
        let package = store.join(ID);
        std::fs::create_dir_all(package.join("ui")).unwrap();
        std::fs::write(package.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        std::fs::write(package.join("ui/index.html"), b"<p>").unwrap();
        std::fs::write(
            package.join(MANIFEST_FILE),
            format!(
                "[package]\nid = \"{manifest_id}\"\nname = \"T\"\nversion = \"1.2.3\"\n\
                 capabilities = [\"sys:notifications\"]\n\
                 [runtime]\nmodule = \"{module}\"\n[ui]\nentry = \"ui/index.html\"\n"
            ),
        )
        .unwrap();
        // SAFETY: callers hold env_lock, which serialises environment changes.
        unsafe { std::env::set_var("WEFT_APP_STORE", &store) };
        store
    }

    fn finish(store: &Path) {
        // SAFETY: as in `store`.
        unsafe {
            std::env::remove_var("WEFT_APP_STORE");
            std::env::remove_var("WEFT_MOUNT_HELPER");
        }
        let _ = std::fs::remove_dir_all(store);
    }

    fn refusal(result: Result<LaunchPackage, Refusal>) -> Refusal {
        match result {
            Ok(package) => panic!("resolved {}", package.root.display()),
            Err(refusal) => refusal,
        }
    }

    #[test]
    fn a_directory_install_resolves_to_its_own_files() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("dir", ID, "app.wasm");
        let package = resolve(ID).unwrap();
        finish(&store);
        assert_eq!(package.root, store.join(ID));
        assert_eq!(package.module, store.join(ID).join("app.wasm"));
        assert_eq!(package.ui_entry, store.join(ID).join("ui/index.html"));
        assert_eq!(package.version, "1.2.3");
        assert_eq!(package.capabilities, ["sys:notifications"]);
        assert!(package.image.is_none());
    }

    #[test]
    fn an_image_in_a_later_store_is_not_shadowed_by_a_directory() {
        let _env = crate::tests::env_lock().blocking_lock();
        crate::tests::use_test_runtime_dir();
        // A directory install in the user store, an incomplete image in the
        // system store: the image is chosen, and refused.
        let user = store("shadow_user", ID, "app.wasm");
        let system = std::env::temp_dir().join(format!(
            "weft_appd_launch_shadow_system_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&system);
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(ImageFiles::in_store(&system, ID).image, b"image").unwrap();
        let refused = refusal(resolve_in(ID, &[user.clone(), system.clone()]));
        finish(&user);
        let _ = std::fs::remove_dir_all(&system);
        assert_eq!(refused.code, 403, "{}", refused.message);
        assert!(
            refused.message.contains("verified image"),
            "{}",
            refused.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_package_root_or_manifest_is_refused() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("linked", ID, "app.wasm");
        let elsewhere =
            std::env::temp_dir().join(format!("weft_appd_launch_elsewhere_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&elsewhere);
        std::fs::rename(store.join(ID), &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, store.join(ID)).unwrap();
        let root_link = refusal(resolve(ID));

        std::fs::remove_file(store.join(ID)).unwrap();
        std::fs::rename(&elsewhere, store.join(ID)).unwrap();
        let manifest = store.join(ID).join(MANIFEST_FILE);
        std::fs::rename(&manifest, store.join("manifest.toml")).unwrap();
        std::os::unix::fs::symlink(store.join("manifest.toml"), &manifest).unwrap();
        let manifest_link = refusal(resolve(ID));
        finish(&store);
        for refused in [root_link, manifest_link] {
            assert_eq!(refused.code, 403, "{}", refused.message);
            assert!(
                refused.message.contains("not a link"),
                "{}",
                refused.message
            );
        }
    }

    #[test]
    fn a_package_declaring_another_id_is_refused() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("id", "org.weft.test.other", "app.wasm");
        let refused = refusal(resolve(ID));
        finish(&store);
        assert_eq!(refused.code, 403);
        assert!(
            refused.message.contains("org.weft.test.other"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn entries_outside_the_package_are_refused() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("entry", ID, "../escape.wasm");
        std::fs::write(store.join("escape.wasm"), b"\0asm").unwrap();
        let refused = refusal(resolve(ID));
        finish(&store);
        assert_eq!(refused.code, 403);
        assert!(
            refused.message.contains("runtime.module"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn missing_packages_and_invalid_ids_are_refused() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("missing", ID, "app.wasm");
        let missing = refusal(resolve("org.weft.test.absent"));
        let invalid = refusal(resolve("../etc"));
        finish(&store);
        assert_eq!(missing.code, 404);
        assert_eq!(invalid.code, 400);
    }

    #[test]
    fn an_image_that_cannot_be_used_never_falls_back_to_the_directory() {
        let _env = crate::tests::env_lock().blocking_lock();
        crate::tests::use_test_runtime_dir();
        let store = store("image", ID, "app.wasm");
        let files = ImageFiles::in_store(&store, ID);

        // Only the image: its companions are missing.
        std::fs::write(&files.image, b"image").unwrap();
        // SAFETY: as in `store`.
        unsafe { std::env::set_var("WEFT_MOUNT_HELPER", "/bin/true") };
        let partial = refusal(resolve(ID));

        // All three files, but the helper fails to mount them.
        std::fs::write(&files.hash_tree, b"tree").unwrap();
        std::fs::write(&files.root_hash, "ab".repeat(32)).unwrap();
        unsafe { std::env::set_var("WEFT_MOUNT_HELPER", "/bin/false") };
        let failed = refusal(resolve(ID));

        // The helper is not installed.
        unsafe { std::env::set_var("WEFT_MOUNT_HELPER", "/nonexistent/weft-mount-helper") };
        let absent = refusal(resolve(ID));

        // A root hash that is not one.
        std::fs::write(&files.root_hash, "not a hash").unwrap();
        unsafe { std::env::set_var("WEFT_MOUNT_HELPER", "/bin/true") };
        let malformed = refusal(resolve(ID));

        let mountpoints = std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("weft/mnt"))
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .map_or(0, Iterator::count);
        finish(&store);

        for refused in [&partial, &malformed] {
            assert_eq!(refused.code, 403, "{}", refused.message);
        }
        for refused in [&failed, &absent] {
            assert_eq!(refused.code, 500, "{}", refused.message);
        }
        for refused in [&partial, &failed, &absent, &malformed] {
            assert!(
                refused.message.contains("verified image"),
                "{}",
                refused.message
            );
        }
        assert!(partial.message.contains(".app.hash"), "{}", partial.message);
        assert_eq!(mountpoints, 0, "a failed mount left its mountpoint behind");
    }
}
