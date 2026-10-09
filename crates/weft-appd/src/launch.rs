//! The package a session runs, resolved once before anything is started.
//!
//! The runtime and the app shell receive the exact files resolved here and
//! never look for the package themselves, so the component, the UI and the
//! capabilities a session is granted all come from the same package root.
//!
//! Before anything starts, the package must still be what its owner
//! installed: a package recorded as a publisher's must carry that
//! publisher's valid signature over its current content, and a package with
//! no record must be signed by a trusted key, which is then recorded as its
//! owner. Development content, recorded as such by `weft-pack install
//! --dev`, runs unverified.

use std::path::{Path, PathBuf};

use weft_ipc_types::manifest::{MANIFEST_FILE, Manifest, ManifestError, entry_path};
use weft_ipc_types::package::{ImageFiles, is_valid_app_id};
use weft_ipc_types::trust::{
    Inventory, Owner, PublisherKey, TrustError, TrustStore, lock_owner, owner_record_path,
    read_owner, read_signature, write_owner,
};

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
    /// The lock that keeps an installed revision from being removed while
    /// the session runs from it.
    pub(crate) pin: Option<weft_ipc_types::store::Pin>,
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
    // A store whose entry for the app cannot be used does not hide the app
    // in a later store, as it does not in the list of installed apps; the
    // first such problem is reported when no store has the app.
    let mut unusable = None;
    for store in stores {
        let active = match weft_ipc_types::store::active(store, app_id) {
            Ok(Some(active)) => active,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(app_id, error = %e, "package entry skipped");
                unusable.get_or_insert(match e {
                    weft_ipc_types::store::StoreError::NotAPackage(_) => {
                        Refusal::new(403, e.to_string())
                    }
                    weft_ipc_types::store::StoreError::Io(..) => Refusal::new(500, e.to_string()),
                });
                continue;
            }
        };
        let dir = active.dir().to_path_buf();
        // A directory left with only app data by an earlier layout is not a
        // package.
        if std::fs::symlink_metadata(dir.join(MANIFEST_FILE)).is_err() {
            continue;
        }
        // The session runs from the revision itself, not the link, so an
        // update or rollback that switches the link while it runs leaves it
        // on the bytes it started with.
        let pin = weft_ipc_types::store::pin(&dir).map_err(|e| {
            Refusal::new(
                500,
                format!("cannot hold {} while it runs: {e}", dir.display()),
            )
        })?;
        let mut package = from_root(app_id, dir, None)?;
        package.pin = Some(pin);
        return Ok(package);
    }
    Err(unusable.unwrap_or_else(|| Refusal::new(404, format!("package {app_id} is not installed"))))
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
    // The manifest is read once; the same bytes are parsed and checked
    // against the signed inventory, so the capabilities and entries a
    // session gets are the ones the owner signed.
    let manifest_path = root.join(MANIFEST_FILE);
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|e| Refusal::new(500, format!("cannot read {}: {e}", manifest_path.display())))?;
    let manifest = Manifest::parse(&manifest_path, &manifest_bytes).map_err(|e| match e {
        ManifestError::Io(..) | ManifestError::Parse(..) => Refusal::new(403, e.to_string()),
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
    verify_owner(app_id, &root, &manifest_bytes)?;
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
        pin: None,
    })
}

/// Checks that the package at `root`, whose manifest was read as
/// `manifest_bytes`, is what its owner installed. A trusted package with no
/// owner record is recorded as its signer's on its first launch, so its app
/// data stays with that publisher; existing app data with no recorded owner
/// is not handed to whichever publisher launches first.
fn verify_owner(app_id: &str, root: &Path, manifest_bytes: &[u8]) -> Result<(), Refusal> {
    // Errors in the host's trust store or owner records are the host's, not
    // the package's.
    let host = |e: TrustError| Refusal::new(500, e.to_string());
    let package = |e: TrustError| match e {
        TrustError::Io(..) => Refusal::new(500, e.to_string()),
        _ => Refusal::new(403, e.to_string()),
    };
    let data_home = weft_ipc_types::package::data_home().ok_or_else(|| {
        Refusal::new(
            500,
            "cannot locate the data home holding package owner records",
        )
    })?;
    let record = owner_record_path(&data_home, app_id);
    let owner = read_owner(&record).map_err(host)?;
    if owner == Some(Owner::Development) {
        return Ok(());
    }
    let trusted = TrustStore::load(&TrustStore::directories()).map_err(host)?;
    let inventory = Inventory::read(root).map_err(package)?;
    if !inventory.holds(MANIFEST_FILE, manifest_bytes) {
        return Err(Refusal::new(
            403,
            format!("{} changed while it was checked", root.display()),
        ));
    }
    let digest = inventory.digest();
    let signature = read_signature(root).map_err(package)?;
    let signer = signature.and_then(|sig| trusted.signer_of(&digest, &sig));
    match (owner, signer) {
        (Some(Owner::Verified(key)), Some(signer)) if key == signer => Ok(()),
        (Some(Owner::Verified(key)), _) => Err(Refusal::new(
            403,
            format!(
                "{} is not signed by its owner, publisher {}, or that key is no longer \
                 trusted; the package was changed or its key was withdrawn",
                root.display(),
                key.to_hex()
            ),
        )),
        (_, None) => Err(Refusal::new(
            403,
            format!(
                "{} has no recorded owner and is not signed by a trusted key; install it \
                 with weft-pack",
                root.display()
            ),
        )),
        (_, Some(signer)) => record_first_owner(app_id, &data_home, &record, signer),
    }
}

/// Records `signer` as the owner of `app_id` on the first launch of a trusted
/// package that has no owner record and no app data yet.
fn record_first_owner(
    app_id: &str,
    data_home: &Path,
    record: &Path,
    signer: PublisherKey,
) -> Result<(), Refusal> {
    let host = |e: TrustError| Refusal::new(500, e.to_string());
    let _lock = lock_owner(data_home, app_id).map_err(host)?;
    match read_owner(record).map_err(host)? {
        Some(Owner::Verified(key)) if key == signer => return Ok(()),
        Some(existing) => {
            return Err(Refusal::new(
                403,
                format!(
                    "{app_id} belongs to {existing}, not publisher {}",
                    signer.to_hex()
                ),
            ));
        }
        None => {}
    }
    // Data in the earlier layout counts: it would be moved to this app's
    // data directory when the session starts.
    let home = weft_ipc_types::package::home_dir()
        .ok_or_else(|| Refusal::new(500, "HOME is not set to an absolute path"))?;
    match weft_ipc_types::package::existing_app_data(data_home, &home, app_id) {
        Err(e) => Err(Refusal::new(
            500,
            format!("cannot inspect the app data of {app_id}: {e}"),
        )),
        Ok(Some(data)) => Err(Refusal::new(
            403,
            format!(
                "{} holds app data for {app_id} with no recorded owner, so it is not given to \
                 publisher {}. To give it to this publisher, install the package into the user \
                 store with weft-pack install --claim-data (uninstalling it there first); \
                 otherwise move the data aside",
                data.display(),
                signer.to_hex()
            ),
        )),
        Ok(None) => write_owner(record, Owner::Verified(signer)).map_err(host),
    }
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
            pin: None,
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

    /// HOME as the test process found it, restored by `finish`.
    static HOME_AT_START: std::sync::OnceLock<Option<std::ffi::OsString>> =
        std::sync::OnceLock::new();

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
        // The fixture is unsigned development content, recorded as such.
        weft_ipc_types::trust::write_owner(
            &owner_record_path(&store.join("share"), ID),
            Owner::Development,
        )
        .unwrap();
        HOME_AT_START.get_or_init(|| std::env::var_os("HOME"));
        // SAFETY: callers hold env_lock, which serialises environment changes.
        unsafe {
            std::env::set_var("WEFT_APP_STORE", &store);
            std::env::set_var("XDG_DATA_HOME", store.join("share"));
            std::env::set_var("HOME", store.join("home"));
        }
        store
    }

    fn finish(store: &Path) {
        // SAFETY: as in `store`.
        unsafe {
            std::env::remove_var("WEFT_APP_STORE");
            std::env::remove_var("WEFT_MOUNT_HELPER");
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("WEFT_TRUSTED_KEYS");
            match HOME_AT_START.get() {
                Some(Some(home)) => std::env::set_var("HOME", home),
                _ => std::env::remove_var("HOME"),
            }
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
        // Only a link to one of the app's own revisions is followed.
        assert_eq!(root_link.code, 403, "{}", root_link.message);
        assert!(
            root_link
                .message
                .contains("nor a link to one of its revisions"),
            "{}",
            root_link.message
        );
        assert_eq!(manifest_link.code, 403, "{}", manifest_link.message);
        assert!(
            manifest_link.message.contains("not a link"),
            "{}",
            manifest_link.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_session_runs_from_its_revision_and_holds_it() {
        use weft_ipc_types::store::{link_target, revisions_of, try_claim};
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("revision", ID, "app.wasm");
        let revision = "ab".repeat(32);
        let dir = revisions_of(&store, ID).join(&revision);
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::rename(store.join(ID), &dir).unwrap();
        std::os::unix::fs::symlink(link_target(ID, &revision), store.join(ID)).unwrap();
        let package = resolve(ID).unwrap();
        let held = try_claim(&dir).unwrap().is_none();
        let root = package.root.clone();
        let module = package.module.clone();
        drop(package);
        let released = try_claim(&dir).unwrap().is_some();
        finish(&store);
        // The children get the revision's own paths, not the link's.
        assert_eq!(root, dir);
        assert_eq!(module, dir.join("app.wasm"));
        assert!(held, "the revision is not held while the session runs");
        assert!(released, "the revision is still held after the session");
    }

    const DEMO: &str = "org.weft.demo.counter";

    /// A store holding a copy of the signed Counter demo, with the demo key
    /// trusted when `trusted`. Callers hold env_lock.
    fn demo_store(name: &str, trusted: bool) -> PathBuf {
        let store = store(name, ID, "app.wasm");
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        copy_tree(&examples.join(DEMO), &store.join(DEMO));
        let keys = store.join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        if trusted {
            std::fs::copy(examples.join("keys/weft-sign.pub"), keys.join("demo.pub")).unwrap();
        }
        // SAFETY: as in `store`.
        unsafe { std::env::set_var("WEFT_TRUSTED_KEYS", &keys) };
        store
    }

    fn copy_tree(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), to).unwrap();
            }
        }
    }

    fn record(store: &Path, app_id: &str, owner: Owner) {
        weft_ipc_types::trust::write_owner(&owner_record_path(&store.join("share"), app_id), owner)
            .unwrap();
    }

    fn demo_key() -> weft_ipc_types::trust::PublisherKey {
        weft_ipc_types::trust::PublisherKey::from_file(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/keys/weft-sign.pub"),
        )
        .unwrap()
    }

    #[test]
    fn a_package_signed_by_its_owner_launches() {
        let _env = crate::tests::env_lock().blocking_lock();
        // Recorded as the publisher's, and with no record but a trusted
        // signature.
        let store = demo_store("signed", true);
        let first = resolve(DEMO).map(|p| p.root);
        let owner = read_owner(&owner_record_path(&store.join("share"), DEMO)).unwrap();
        let again = resolve(DEMO).map(|p| p.root);
        finish(&store);
        assert_eq!(first.ok(), Some(store.join(DEMO)));
        assert_eq!(owner, Some(Owner::Verified(demo_key())));
        assert_eq!(again.ok(), Some(store.join(DEMO)));
    }

    #[test]
    fn unowned_app_data_is_not_given_to_the_first_publisher() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = demo_store("unowned_data", true);
        let data = weft_ipc_types::package::app_data_dir(&store.join("share"), DEMO);
        std::fs::create_dir_all(&data).unwrap();
        let refused = refusal(resolve(DEMO));
        let owner = read_owner(&owner_record_path(&store.join("share"), DEMO)).unwrap();
        finish(&store);
        assert_eq!(refused.code, 403, "{}", refused.message);
        assert!(
            refused.message.contains("--claim-data"),
            "{}",
            refused.message
        );
        assert_eq!(owner, None);
    }

    #[test]
    fn unowned_data_in_the_earlier_layout_is_not_given_away() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = demo_store("legacy_data", true);
        let legacy = weft_ipc_types::package::legacy_app_data_dir(&store.join("home"), DEMO);
        std::fs::create_dir_all(&legacy).unwrap();
        let refused = refusal(resolve(DEMO));
        let owner = read_owner(&owner_record_path(&store.join("share"), DEMO)).unwrap();
        finish(&store);
        assert_eq!(refused.code, 403, "{}", refused.message);
        assert!(
            refused.message.contains("no recorded owner"),
            "{}",
            refused.message
        );
        assert_eq!(owner, None);
    }

    #[test]
    fn a_first_launch_without_a_usable_home_records_nothing() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = demo_store("no_home", true);
        // SAFETY: env_lock is held; finish restores HOME.
        unsafe { std::env::set_var("HOME", "relative") };
        let refused = refusal(resolve(DEMO));
        let owner = read_owner(&owner_record_path(&store.join("share"), DEMO)).unwrap();
        finish(&store);
        assert_eq!(refused.code, 500, "{}", refused.message);
        assert_eq!(owner, None);
    }

    #[test]
    fn only_the_signed_manifest_is_accepted() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = demo_store("manifest", true);
        record(&store, DEMO, Owner::Verified(demo_key()));
        let root = store.join(DEMO);
        let signed = std::fs::read(root.join(MANIFEST_FILE)).unwrap();
        let mut other = signed.clone();
        other.extend_from_slice(b"\n# other\n");
        let accepted = verify_owner(DEMO, &root, &signed);
        let refused = verify_owner(DEMO, &root, &other).err();
        finish(&store);
        assert!(accepted.is_ok());
        let refused = refused.expect("a manifest that was not signed was accepted");
        assert_eq!(refused.code, 403, "{}", refused.message);
    }

    #[test]
    fn development_content_does_not_depend_on_the_trust_store() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("dev_store", ID, "app.wasm");
        let keys = store.join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        std::fs::write(keys.join("broken.pub"), "not a key").unwrap();
        // SAFETY: as in `store`.
        unsafe { std::env::set_var("WEFT_TRUSTED_KEYS", &keys) };
        let resolved = resolve(ID).map(|p| p.root);
        finish(&store);
        assert_eq!(resolved.ok(), Some(store.join(ID)));
    }

    #[test]
    fn a_damaged_trust_store_or_record_is_a_host_error() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = demo_store("damaged", true);
        std::fs::write(store.join("keys/broken.pub"), "not a key").unwrap();
        let bad_key = refusal(resolve(DEMO));
        std::fs::remove_file(store.join("keys/broken.pub")).unwrap();
        let record = owner_record_path(&store.join("share"), DEMO);
        std::fs::write(&record, "nonsense\n").unwrap();
        let bad_record = refusal(resolve(DEMO));
        finish(&store);
        for refused in [bad_key, bad_record] {
            assert_eq!(refused.code, 500, "{}", refused.message);
        }
    }

    #[test]
    fn a_package_changed_after_installation_is_refused() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = demo_store("changed", true);
        record(&store, DEMO, Owner::Verified(demo_key()));
        let page = store.join(DEMO).join("ui/index.html");
        let mut text = std::fs::read_to_string(&page).unwrap();
        text.push_str("<script>changed</script>");
        std::fs::write(&page, text).unwrap();
        let changed = refusal(resolve(DEMO));
        // Without a record, the changed package matches no trusted key.
        std::fs::remove_file(owner_record_path(&store.join("share"), DEMO)).unwrap();
        let unrecorded = refusal(resolve(DEMO));
        finish(&store);
        assert_eq!(changed.code, 403, "{}", changed.message);
        assert!(
            changed.message.contains("not signed by its owner"),
            "{}",
            changed.message
        );
        assert_eq!(unrecorded.code, 403, "{}", unrecorded.message);
        assert!(
            unrecorded.message.contains("no recorded owner"),
            "{}",
            unrecorded.message
        );
    }

    #[test]
    fn a_withdrawn_key_no_longer_launches_its_packages() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = demo_store("withdrawn", false);
        record(&store, DEMO, Owner::Verified(demo_key()));
        let refused = refusal(resolve(DEMO));
        finish(&store);
        assert_eq!(refused.code, 403, "{}", refused.message);
        assert!(refused.message.contains("no longer"), "{}", refused.message);
    }

    #[test]
    fn unsigned_content_needs_a_development_record() {
        let _env = crate::tests::env_lock().blocking_lock();
        let store = store("unrecorded", ID, "app.wasm");
        std::fs::remove_file(owner_record_path(&store.join("share"), ID)).unwrap();
        let refused = refusal(resolve(ID));
        // Another owner's record does not admit it either.
        record(&store, ID, Owner::Verified(demo_key()));
        let other_owner = refusal(resolve(ID));
        finish(&store);
        for refused in [refused, other_owner] {
            assert_eq!(refused.code, 403, "{}", refused.message);
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
