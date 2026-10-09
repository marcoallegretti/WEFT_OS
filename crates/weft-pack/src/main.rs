use std::path::{Path, PathBuf};

use anyhow::Context;
use weft_ipc_types::manifest::{Manifest, entry_path};
use weft_ipc_types::package::ImageFiles;
use weft_ipc_types::trust::{Owner, PublisherKey, TrustStore};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("check") => {
            let dir = args.get(2).context("usage: weft-pack check <dir>")?;
            let result = check_package(Path::new(dir))?;
            println!("{result}");
        }
        Some("info") => {
            let dir = args.get(2).context("usage: weft-pack info <dir>")?;
            let manifest = load_manifest(Path::new(dir))?;
            print_info(&manifest);
        }
        Some("install") => {
            let usage = "usage: weft-pack install <dir|archive> [--dev] [--claim-data]";
            let dir = args.get(2).context(usage)?;
            let mut mode = InstallMode::Verified;
            let mut claim_data = false;
            for arg in &args[3..] {
                match arg.as_str() {
                    "--dev" => mode = InstallMode::Development,
                    "--claim-data" => claim_data = true,
                    other => anyhow::bail!("unexpected argument '{other}'; {usage}"),
                }
            }
            install_package(Path::new(dir), mode, claim_data)?;
        }
        Some("uninstall") => {
            let app_id = args.get(2).context("usage: weft-pack uninstall <app_id>")?;
            uninstall_package(app_id)?;
        }
        Some("list") => {
            list_installed();
        }
        Some("build-image") => {
            let dir = args.get(2).context("usage: weft-pack build-image <dir>")?;
            let out = args
                .windows(2)
                .find(|w| w[0] == "--out")
                .map(|w| w[1].as_str());
            build_image(Path::new(dir), out.map(Path::new))?;
        }
        Some("build-verity") => {
            let img = args.get(2).context("usage: weft-pack build-verity <img>")?;
            let out = args
                .windows(2)
                .find(|w| w[0] == "--out")
                .map(|w| w[1].as_str());
            build_verity(Path::new(img), out.map(Path::new))?;
        }
        Some("bundle") => {
            let dir = args.get(2).context("usage: weft-pack bundle <dir>")?;
            let out = args
                .windows(2)
                .find(|w| w[0] == "--out")
                .map(|w| w[1].as_str());
            bundle_package(Path::new(dir), out.map(Path::new))?;
        }
        Some("unbundle") => {
            let archive = args.get(2).context("usage: weft-pack unbundle <archive>")?;
            let out = args
                .windows(2)
                .find(|w| w[0] == "--out")
                .map(|w| w[1].as_str())
                .unwrap_or(".");
            unbundle_package(Path::new(archive), Path::new(out))?;
        }
        Some("generate-key") => {
            let out = args.get(2).map(String::as_str).unwrap_or(".");
            generate_key(Path::new(out))?;
        }
        Some("sign") => {
            let dir = args
                .get(2)
                .context("usage: weft-pack sign <dir> --key <keyfile>")?;
            let key = args
                .windows(2)
                .find(|w| w[0] == "--key")
                .map(|w| &w[1])
                .context("missing --key <keyfile>")?;
            sign_package(Path::new(dir), Path::new(key))?;
        }
        Some("verify") => {
            let dir = args
                .get(2)
                .context("usage: weft-pack verify <dir> --key <pubkeyfile>")?;
            let key = args
                .windows(2)
                .find(|w| w[0] == "--key")
                .map(|w| &w[1])
                .context("missing --key <pubkeyfile>")?;
            let ok = verify_package(Path::new(dir), &PublisherKey::from_file(Path::new(key))?)?;
            if ok {
                println!("OK");
            } else {
                eprintln!("INVALID signature");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  weft-pack check        <dir>             validate a package directory");
            eprintln!("  weft-pack info         <dir>             print package metadata");
            eprintln!("  weft-pack install      <dir> [--dev] [--claim-data]");
            eprintln!(
                "                                           install a package signed by a trusted key"
            );
            eprintln!(
                "                                           --dev: as unsigned development content"
            );
            eprintln!(
                "                                           --claim-data: adopt app data with no owner"
            );
            eprintln!("  weft-pack uninstall    <app_id>          remove installed package");
            eprintln!("  weft-pack list                           list installed packages");
            eprintln!(
                "  weft-pack build-image   <dir> [--out <img>] create EROFS image with mkfs.erofs"
            );
            eprintln!("  weft-pack build-verity   <img> [--out <hash>] add dm-verity hash tree");
            eprintln!("  weft-pack bundle        <dir> [--out <dir>] create .app.tar.zst archive");
            eprintln!("  weft-pack unbundle      <archive> [--out <dir>] extract .app.tar.zst");
            eprintln!("  weft-pack generate-key [<outdir>]        generate Ed25519 keypair");
            eprintln!("  weft-pack sign         <dir> --key <key> sign package with private key");
            eprintln!("  weft-pack verify       <dir> --key <pub> verify package signature");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn check_package(dir: &Path) -> anyhow::Result<String> {
    let mut errors: Vec<String> = Vec::new();

    let manifest = match load_manifest(dir) {
        Ok(m) => Some(m),
        Err(e) => {
            errors.push(format!("wapp.toml: {e}"));
            None
        }
    };

    // A package holds only directories and regular files: its signature
    // covers exactly those, and installation copies exactly those.
    if let Err(e) = weft_ipc_types::trust::content_digest(dir) {
        errors.push(e.to_string());
    }

    // Earlier versions kept app data in `<package>/data`; the name stays
    // reserved so package content is never mistaken for user data.
    if std::fs::symlink_metadata(dir.join("data")).is_ok() {
        errors.push("a top-level 'data' entry is reserved and cannot be part of a package".into());
    }

    if let Some(ref m) = manifest {
        if !weft_ipc_types::package::is_valid_app_id(&m.package.id) {
            errors.push(format!(
                "package.id '{}' does not match required pattern",
                m.package.id
            ));
        }
        if m.package.name.is_empty() {
            errors.push("package.name is empty".into());
        }
        if m.package.name.len() > 64 {
            errors.push(format!(
                "package.name exceeds 64 characters ({})",
                m.package.name.len()
            ));
        }

        match entry_path(dir, &m.runtime.module) {
            Err(e) => errors.push(format!("runtime.module: {e}")),
            Ok(wasm_path) if !is_wasm_module(&wasm_path) => errors.push(format!(
                "runtime.module '{}' is not a valid Wasm module (bad magic bytes)",
                wasm_path.display()
            )),
            Ok(_) => {}
        }

        if let Err(e) = entry_path(dir, &m.ui.entry) {
            errors.push(format!("ui.entry: {e}"));
        }

        for cap in m.package.capabilities.iter().flatten() {
            if let Err(e) = cap.parse::<weft_ipc_types::capability::Capability>() {
                errors.push(e.to_string());
            }
        }
    }

    if errors.is_empty() {
        Ok("OK".into())
    } else {
        Err(anyhow::anyhow!("{}", errors.join("\n")))
    }
}

fn load_manifest(dir: &Path) -> anyhow::Result<Manifest> {
    Ok(Manifest::read(dir)?)
}

fn print_info(m: &Manifest) {
    println!("id:      {}", m.package.id);
    println!("name:    {}", m.package.name);
    println!("version: {}", m.package.version);
    if let Some(ref d) = m.package.description {
        println!("desc:    {d}");
    }
    if let Some(ref a) = m.package.author {
        println!("author:  {a}");
    }
    println!("module:  {}", m.runtime.module);
    println!("ui:      {}", m.ui.entry);
    if let Some(ref caps) = m.package.capabilities {
        for cap in caps {
            println!("cap:     {cap}");
        }
    }
}

fn is_wasm_module(path: &Path) -> bool {
    const MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
    let mut buf = [0u8; 4];
    match std::fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read_exact(&mut buf)
                .map(|_| buf == MAGIC)
                .unwrap_or(false)
        }
        Err(_) => false,
    }
}

fn resolve_install_root() -> anyhow::Result<PathBuf> {
    if let Ok(explicit) = std::env::var("WEFT_APP_STORE") {
        return Ok(PathBuf::from(explicit));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("weft")
            .join("apps"));
    }
    anyhow::bail!("cannot determine install root: HOME and WEFT_APP_STORE are both unset")
}

/// How a package is admitted to the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallMode {
    /// The package must be signed by a key in the trust store.
    Verified,
    /// Unsigned or untrusted content, chosen explicitly by a developer and
    /// recorded as development content.
    Development,
}

fn install_package(path: &Path, mode: InstallMode, claim_data: bool) -> anyhow::Result<()> {
    let root = resolve_install_root()?;
    install_into(path, &root, mode, claim_data)
}

#[cfg(test)]
fn install_package_to(path: &Path, store_root: &Path, mode: InstallMode) -> anyhow::Result<()> {
    install_into(path, store_root, mode, false)
}

/// Installs the package at `path` into `store_root`. App data that exists
/// for the ID without a recorded owner, as left by an installation that
/// predates owner records, goes to this package's owner only with
/// `claim_data`.
fn install_into(
    path: &Path,
    store_root: &Path,
    mode: InstallMode,
    claim_data: bool,
) -> anyhow::Result<()> {
    create_store(store_root)?;
    if !(path.extension().is_some_and(|e| e == "zst" || e == "tar")
        || path.to_string_lossy().ends_with(".app.tar.zst"))
    {
        return install_dir(path, store_root, mode, claim_data);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".app.tar.zst"))
        .filter(|n| weft_ipc_types::package::is_valid_app_id(n))
        .context("the archive must be named <app_id>.app.tar.zst")?;
    // A fresh private directory in the store, removed whatever happens, so
    // nothing can be planted in it and nothing is left behind.
    let unpacked = private_scratch_dir(store_root, ".unpack")?;
    let result = unbundle_package(path, &unpacked)
        .and_then(|()| install_dir(&unpacked.join(name), store_root, mode, claim_data));
    let _ = std::fs::remove_dir_all(&unpacked);
    result
}

/// Creates the store and any missing parents at 0755 whatever the umask, so
/// a store created by a root install is readable by every user.
fn create_store(store_root: &Path) -> anyhow::Result<()> {
    let mut missing: Vec<&Path> = store_root
        .ancestors()
        .take_while(|dir| std::fs::symlink_metadata(dir).is_err())
        .collect();
    missing.reverse();
    for dir in missing {
        match std::fs::create_dir(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            other => other.with_context(|| format!("create {}", dir.display()))?,
        }
        #[cfg(unix)]
        std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .with_context(|| format!("set the mode of {}", dir.display()))?;
    }
    anyhow::ensure!(
        store_root.is_dir(),
        "{} is not a directory",
        store_root.display()
    );
    Ok(())
}

/// Creates a new directory with a random name in `parent`, accessible only to
/// the user.
fn private_scratch_dir(parent: &Path, prefix: &str) -> anyhow::Result<PathBuf> {
    let dir = parent.join(format!(
        "{prefix}-{}",
        hex::encode(rand::random::<[u8; 8]>())
    ));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&dir)
        .with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn install_dir(
    dir: &Path,
    store_root: &Path,
    mode: InstallMode,
    claim_data: bool,
) -> anyhow::Result<()> {
    check_package(dir)?;
    // The package is copied into the store first and every decision, the app
    // ID included, is made on that copy, so the bytes that are checked and
    // trusted are the bytes that become installed. Only then does one rename
    // make it available.
    let staging = store_root.join(format!(
        ".staging-{}",
        hex::encode(rand::random::<[u8; 8]>())
    ));
    let result = dir
        .canonicalize()
        .with_context(|| format!("resolve {}", dir.display()))
        .and_then(|source| copy_dir(&source, &staging))
        .with_context(|| format!("copy {} -> {}", dir.display(), staging.display()))
        .and_then(|()| install_staged(&staging, store_root, mode, claim_data));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn install_staged(
    staging: &Path,
    store_root: &Path,
    mode: InstallMode,
    claim_data: bool,
) -> anyhow::Result<()> {
    check_package(staging)?;
    let manifest = load_manifest(staging)?;
    let app_id = &manifest.package.id;
    let dest = store_root.join(app_id);
    let data_home = weft_ipc_types::package::data_home()
        .context("cannot locate the data home to record who owns the app ID")?;
    // Installations and uninstallations of one ID run one at a time, so the
    // owner record always describes the package that is placed.
    let _lock = weft_ipc_types::trust::lock_owner(&data_home, app_id)?;
    // An earlier weft-appd could leave only app data, or an empty directory,
    // where the package now goes; move the data to its own location so it
    // neither blocks nor joins the package.
    if dest.is_dir() && only_holds_app_data(&dest)? {
        if std::fs::symlink_metadata(dest.join("data")).is_ok() && is_user_store(store_root) {
            move_app_data_out(app_id, &dest)?;
        }
        let _ = std::fs::remove_dir(&dest);
    }
    if dest.exists() {
        anyhow::bail!(
            "package '{}' is already installed at {}; remove it first",
            app_id,
            dest.display()
        );
    }
    let admission = admit(staging, app_id, mode, &data_home, claim_data)?;
    let owner = admission.owner;
    place(staging, &dest, admission)?;
    println!("installed {} -> {} ({owner})", app_id, dest.display());
    Ok(())
}

/// The owner a staged package was admitted under, and the owner record this
/// installation created, if any.
struct Admission {
    owner: Owner,
    created_record: Option<PathBuf>,
}

/// Makes the staged package available at `dest`. If that fails, the staging
/// copy is removed, and so is an owner record this installation created, so
/// a failed first installation leaves no claim on the app ID.
fn place(staging: &Path, dest: &Path, admission: Admission) -> anyhow::Result<()> {
    if let Err(e) = weft_ipc_types::package::rename_no_replace(staging, dest) {
        let _ = std::fs::remove_dir_all(staging);
        if let Some(record) = &admission.created_record {
            let _ = std::fs::remove_file(record);
        }
        return Err(e).with_context(|| format!("move {} -> {}", staging.display(), dest.display()));
    }
    Ok(())
}

/// Establishes who owns the app ID of a checked, staged package:
/// the trusted publisher that signed it, or, only when asked, development.
/// The first installation of an ID records its owner, and every later one
/// must have the same owner. The record is written before the package is
/// placed, so concurrent installations by different owners cannot both win.
fn admit(
    staged: &Path,
    app_id: &str,
    mode: InstallMode,
    data_home: &Path,
    claim_data: bool,
) -> anyhow::Result<Admission> {
    use weft_ipc_types::trust::{owner_record_path, read_owner, write_owner};
    let owner = match mode {
        InstallMode::Development => Owner::Development,
        InstallMode::Verified => {
            let dirs = TrustStore::directories();
            let signer = TrustStore::load(&dirs)?.signer(staged)?;
            Owner::Verified(signer.with_context(|| {
                format!(
                    "{app_id} is not signed by a trusted key (trusted keys are read from {}); \
                     install it with --dev to use it as development content",
                    dirs.iter()
                        .map(|d| d.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?)
        }
    };
    let record = owner_record_path(data_home, app_id);
    let created_record = match read_owner(&record)? {
        Some(existing) if existing == owner => None,
        Some(existing) => anyhow::bail!(
            "{app_id} belongs to {existing}; this package is {owner}. Its app data stays with \
             its owner, so another publisher or a development build cannot take the ID over"
        ),
        None => {
            let data = weft_ipc_types::package::app_data_dir(data_home, app_id);
            let no_data = match std::fs::symlink_metadata(&data) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                Err(e) => return Err(e).with_context(|| format!("inspect {}", data.display())),
                Ok(_) => false,
            };
            if !no_data && !claim_data {
                anyhow::bail!(
                    "{} holds app data for {app_id} with no recorded owner; install with \
                     --claim-data to give it to {owner}, or move it aside",
                    data.display()
                );
            }
            write_owner(&record, owner)?;
            Some(record)
        }
    };
    Ok(Admission {
        owner,
        created_record,
    })
}

fn uninstall_package(app_id: &str) -> anyhow::Result<()> {
    let root = resolve_install_root()?;
    uninstall_package_from(app_id, &root)
}

fn uninstall_package_from(app_id: &str, store_root: &Path) -> anyhow::Result<()> {
    if !weft_ipc_types::package::is_valid_app_id(app_id) {
        anyhow::bail!("'{}' is not a valid app ID", app_id);
    }
    let data_home = weft_ipc_types::package::data_home();
    let _lock = match &data_home {
        Some(home) => Some(weft_ipc_types::trust::lock_owner(home, app_id)?),
        None => None,
    };
    let target = store_root.join(app_id);
    if !target.exists() {
        // An installation interrupted between recording its owner and placing
        // the package leaves a record with nothing installed; with no data
        // either, it holds the ID for nothing and is released.
        if let Some(home) = &data_home
            && !installed_in_another_store(app_id, store_root)
            && release_owner_without_data(home, app_id)
        {
            println!("released {app_id}, which was not installed");
            return Ok(());
        }
        anyhow::bail!(
            "package '{}' is not installed at {}",
            app_id,
            target.display()
        );
    }
    // A `data` directory is never removed with the package: packages cannot
    // ship one, so it holds app data from an earlier version. It is moved to
    // the app data directory when this is the user store and the package
    // declares app data (or its manifest cannot be read); otherwise uninstall
    // stops and leaves everything in place.
    if std::fs::symlink_metadata(target.join("data")).is_ok() {
        let may_hold_app_data = load_manifest(&target).map_or(true, |m| declares_app_data(&m));
        if !(may_hold_app_data && is_user_store(store_root)) {
            anyhow::bail!(
                "{} contains a 'data' directory that may hold app data; move it aside before \
                 uninstalling",
                target.display()
            );
        }
        move_app_data_out(app_id, &target)?;
    }
    // Moving the data removes a package directory it leaves empty.
    match std::fs::remove_dir_all(&target) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("remove {}", target.display()));
        }
        _ => {}
    }
    // The owner record keeps the app ID, and the data with it, for its owner
    // while the data remains; with no data left, the ID is free again.
    if let Some(home) = &data_home
        && !installed_in_another_store(app_id, store_root)
    {
        release_owner_without_data(home, app_id);
    }
    println!("uninstalled {}", app_id);
    Ok(())
}

/// Whether `app_id` is installed in a known store other than `store_root`.
/// Owner records belong to the data home, not to a store, so a record stays
/// while any store this host knows holds the package.
fn installed_in_another_store(app_id: &str, store_root: &Path) -> bool {
    let mut roots = list_installed_roots();
    roots.push(PathBuf::from("/usr/share/weft/apps"));
    roots.extend(std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/weft/apps")));
    roots.sort();
    roots.dedup();
    // Only a package counts, as in `list`: a leftover empty or data-only
    // directory does not hold the ID.
    roots
        .iter()
        .filter(|root| root.as_path() != store_root)
        .any(|root| Manifest::read(&root.join(app_id)).is_ok_and(|m| m.package.id == app_id))
}

/// Removes the owner record of `app_id` when the app has no data, and
/// reports whether a record was removed.
fn release_owner_without_data(data_home: &Path, app_id: &str) -> bool {
    let no_data =
        std::fs::symlink_metadata(weft_ipc_types::package::app_data_dir(data_home, app_id))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    no_data
        && std::fs::remove_file(weft_ipc_types::trust::owner_record_path(data_home, app_id)).is_ok()
}

/// Whether a package directory is empty or contains nothing but `data`.
fn only_holds_app_data(package_dir: &Path) -> anyhow::Result<bool> {
    let entries = std::fs::read_dir(package_dir)
        .with_context(|| format!("read {}", package_dir.display()))?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries.is_empty() || entries == ["data"])
}

/// Whether the manifest declares an app-data capability; only such apps
/// had data kept in the package directory.
fn declares_app_data(manifest: &Manifest) -> bool {
    use weft_ipc_types::capability::Capability;
    manifest
        .package
        .capabilities
        .iter()
        .flatten()
        .any(|c| matches!(c.parse(), Ok(Capability::AppData(_))))
}

/// Whether `store_root` is the user package store, where earlier versions of
/// weft-appd kept app data, however the path is spelled.
fn is_user_store(store_root: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let user_store = home.join(".local/share/weft/apps");
    match (
        std::fs::canonicalize(store_root),
        std::fs::canonicalize(user_store),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Moves app data that an earlier weft-appd kept in `<package_dir>/data` in
/// the user store to the app's data directory, which package operations
/// never remove. Fails, changing nothing, if the data cannot be moved.
fn move_app_data_out(app_id: &str, package_dir: &Path) -> anyhow::Result<()> {
    use weft_ipc_types::package::{
        Migration, app_data_dir, data_home, make_private, migrate_app_data,
    };
    let legacy = package_dir.join("data");
    let data_home = data_home().context("cannot locate the data home to keep app data")?;
    let target = app_data_dir(&data_home, app_id);
    if migrate_app_data(&legacy, &target).with_context(|| format!("keep app data of {app_id}"))?
        == Migration::Moved
    {
        make_private(&target).with_context(|| format!("restrict {}", target.display()))?;
        println!("app data kept at {}", target.display());
    }
    Ok(())
}

fn list_installed_roots() -> Vec<PathBuf> {
    if let Ok(explicit) = std::env::var("WEFT_APP_STORE") {
        return vec![PathBuf::from(explicit)];
    }
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("weft")
                .join("apps"),
        );
    }
    roots.push(PathBuf::from("/usr/share/weft/apps"));
    roots
}

fn list_installed() {
    let mut seen = std::collections::HashSet::new();
    let mut count = 0usize;
    for root in list_installed_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut pkgs: Vec<(String, String, String)> = Vec::new();
        for entry in entries.flatten() {
            let Ok(m) = Manifest::read(&entry.path()) else {
                continue;
            };
            // Only a directory named after the ID its manifest declares is an
            // installed package; staging copies and stray directories are not.
            if entry.file_name().to_str() != Some(m.package.id.as_str())
                || !weft_ipc_types::package::is_valid_app_id(&m.package.id)
            {
                continue;
            }
            if seen.insert(m.package.id.clone()) {
                let development = weft_ipc_types::package::data_home().is_some_and(|home| {
                    matches!(
                        weft_ipc_types::trust::read_owner(
                            &weft_ipc_types::trust::owner_record_path(&home, &m.package.id)
                        ),
                        Ok(Some(Owner::Development))
                    )
                });
                let version = if development {
                    format!("{}  (development)", m.package.version)
                } else {
                    m.package.version
                };
                pkgs.push((m.package.id, m.package.name, version));
            }
        }
        pkgs.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, name, version) in pkgs {
            println!("{id}  {name}  {version}");
            count += 1;
        }
    }
    if count == 0 {
        println!("no packages installed");
    }
}

/// Copies a package directory. Packages hold only directories and regular
/// files, so a symbolic link or special file stops the copy rather than
/// being followed. The walk holds each directory open and opens its entries
/// relative to it without following links, checking that each opened entry
/// has the type, device and inode it had when examined just before the
/// open, so a link or another file swapped in anywhere in the source during
/// the copy cannot redirect it.
/// Modes are not copied: the signature does not cover them, so installed
/// directories are 0755 and files 0644, or 0755 when the source is
/// executable, and nothing installed is writable by other users.
#[cfg(unix)]
fn copy_dir(src: &Path, dst: &Path) -> anyhow::Result<()> {
    use rustix::fs::{CWD, Mode, OFlags};
    let root = rustix::fs::openat(
        CWD,
        src,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("open {}", src.display()))?;
    copy_tree(&root, src, dst)
}

#[cfg(not(unix))]
fn copy_dir(_src: &Path, _dst: &Path) -> anyhow::Result<()> {
    anyhow::bail!("installing packages requires a Unix host")
}

#[cfg(unix)]
fn copy_tree(dir: &rustix::fd::OwnedFd, shown: &Path, dst: &Path) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    // Modes are set explicitly rather than requested at creation, so the
    // installer's umask cannot make an installed package unreadable.
    let set_mode = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("set the mode of {}", path.display()))
    };
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dst)
        .with_context(|| format!("create {}", dst.display()))?;
    set_mode(dst, 0o755)?;
    // The names are read first and the listing closed, so the walk holds one
    // descriptor per directory level.
    let mut names = Vec::new();
    let mut entries =
        rustix::fs::Dir::read_from(dir).with_context(|| format!("read {}", shown.display()))?;
    while let Some(entry) = entries.read() {
        let entry = entry.with_context(|| format!("read {}", shown.display()))?;
        if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
            names.push(entry.file_name().to_owned());
        }
    }
    drop(entries);
    for name in &names {
        let src_path = shown.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        let dst_path = dst.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        let listed = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
            .with_context(|| format!("inspect {}", src_path.display()))?;
        let kind = FileType::from_raw_mode(listed.st_mode);
        let flags = match kind {
            FileType::Directory => OFlags::DIRECTORY,
            FileType::RegularFile => OFlags::NONBLOCK | OFlags::NOCTTY,
            _ => anyhow::bail!(
                "{} is not a regular file or directory; packages cannot contain links",
                src_path.display()
            ),
        };
        let opened = rustix::fs::openat(
            dir,
            name,
            flags | OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .with_context(|| format!("open {}", src_path.display()))?;
        let found = rustix::fs::fstat(&opened)?;
        anyhow::ensure!(
            FileType::from_raw_mode(found.st_mode) == kind
                && found.st_dev == listed.st_dev
                && found.st_ino == listed.st_ino,
            "{} changed while it was copied",
            src_path.display()
        );
        if kind == FileType::Directory {
            copy_tree(&opened, &src_path, &dst_path)?;
        } else {
            let executable = found.st_mode & 0o111 != 0;
            let mut target = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&dst_path)
                .with_context(|| format!("create {}", dst_path.display()))?;
            std::io::copy(&mut std::fs::File::from(opened), &mut target)
                .with_context(|| format!("copy {}", src_path.display()))?;
            target
                .set_permissions(std::fs::Permissions::from_mode(if executable {
                    0o755
                } else {
                    0o644
                }))
                .with_context(|| format!("set the mode of {}", dst_path.display()))?;
        }
    }
    Ok(())
}

fn build_image(dir: &Path, out_path: Option<&Path>) -> anyhow::Result<()> {
    let manifest = load_manifest(dir)?;
    let app_id = &manifest.package.id;
    let output = match out_path {
        Some(p) => p.to_path_buf(),
        None => ImageFiles::in_store(Path::new(""), app_id).image,
    };
    if output.exists() {
        anyhow::bail!("{} already exists", output.display());
    }
    let status = std::process::Command::new("mkfs.erofs")
        .arg(&output)
        .arg(dir)
        .status()
        .context("spawn mkfs.erofs; ensure erofs-utils is installed")?;
    if !status.success() {
        anyhow::bail!("mkfs.erofs failed with status {status}");
    }
    println!("image: {}", output.display());
    Ok(())
}

fn build_verity(img: &Path, hash_out: Option<&Path>) -> anyhow::Result<()> {
    let companions = ImageFiles::for_image(img);
    let hash_path = hash_out
        .map(|p| p.to_path_buf())
        .unwrap_or(companions.hash_tree);
    if hash_path.exists() {
        anyhow::bail!("{} already exists", hash_path.display());
    }
    let output = std::process::Command::new("veritysetup")
        .args([
            "format",
            &img.to_string_lossy(),
            &hash_path.to_string_lossy(),
        ])
        .output()
        .context("spawn veritysetup; ensure cryptsetup-bin is installed")?;
    if !output.status.success() {
        anyhow::bail!(
            "veritysetup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let root_hash = stdout
        .lines()
        .find(|l| l.starts_with("Root hash:"))
        .context("root hash not found in veritysetup output")?;
    let hash_value = root_hash.trim_start_matches("Root hash:").trim();
    let roothash_path = companions.root_hash;
    std::fs::write(&roothash_path, hash_value)
        .with_context(|| format!("write {}", roothash_path.display()))?;
    println!("hash image:  {}", hash_path.display());
    println!("root hash:   {hash_value}");
    println!("roothash:    {}", roothash_path.display());
    Ok(())
}

fn bundle_package(dir: &Path, out_dir: Option<&Path>) -> anyhow::Result<()> {
    let manifest = load_manifest(dir)?;
    let app_id = &manifest.package.id;
    let archive_name = format!("{app_id}.app.tar.zst");
    let dest_dir = out_dir.unwrap_or_else(|| Path::new("."));
    let archive_path = dest_dir.join(&archive_name);
    if archive_path.exists() {
        anyhow::bail!("{} already exists", archive_path.display());
    }
    let file = std::fs::File::create(&archive_path)
        .with_context(|| format!("create {}", archive_path.display()))?;
    let encoder = zstd::Encoder::new(file, 0)
        .context("create zstd encoder")?
        .auto_finish();
    let mut tar = tar::Builder::new(encoder);
    tar.follow_symlinks(false);
    tar.append_dir_all(app_id, dir)
        .with_context(|| format!("append {} to archive", dir.display()))?;
    tar.finish().context("finish tar archive")?;
    println!("bundled: {}", archive_path.display());
    Ok(())
}

fn unbundle_package(archive: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let file =
        std::fs::File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let decoder = zstd::Decoder::new(file).context("create zstd decoder")?;
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(out_dir)
        .with_context(|| format!("unpack to {}", out_dir.display()))?;
    println!("unbundled: {} -> {}", archive.display(), out_dir.display());
    Ok(())
}

fn generate_key(output_dir: &Path) -> anyhow::Result<()> {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    let key_path = output_dir.join("weft-sign.key");
    let pub_path = output_dir.join("weft-sign.pub");
    if key_path.exists() || pub_path.exists() {
        anyhow::bail!(
            "key files already exist in {}; remove them first",
            output_dir.display()
        );
    }
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("create {}", output_dir.display()))?;
    std::fs::write(&key_path, hex::encode(signing_key.to_bytes()))
        .with_context(|| format!("write {}", key_path.display()))?;
    std::fs::write(&pub_path, hex::encode(verifying_key.to_bytes()))
        .with_context(|| format!("write {}", pub_path.display()))?;
    println!("private key: {}", key_path.display());
    println!("public key:  {}", pub_path.display());
    Ok(())
}

fn sign_package(dir: &Path, key_file: &Path) -> anyhow::Result<()> {
    use ed25519_dalek::{Signer, SigningKey};
    let key_hex = std::fs::read_to_string(key_file)
        .with_context(|| format!("read {}", key_file.display()))?;
    let key_bytes: [u8; 32] = hex::decode(key_hex.trim())
        .context("decode signing key: expected 64 hex chars")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must be 32 bytes"))?;
    let signing_key = SigningKey::from_bytes(&key_bytes);
    let digest = weft_ipc_types::trust::content_digest(dir)?;
    let signature = signing_key.sign(&digest);
    let sig_path = dir.join(weft_ipc_types::trust::SIGNATURE_FILE);
    std::fs::write(&sig_path, hex::encode(signature.to_bytes()))
        .with_context(|| format!("write {}", sig_path.display()))?;
    println!("signed: {}", sig_path.display());
    Ok(())
}

fn verify_package(dir: &Path, key: &PublisherKey) -> anyhow::Result<bool> {
    let signature = weft_ipc_types::trust::read_signature(dir)?
        .with_context(|| format!("{} is not signed", dir.display()))?;
    let digest = weft_ipc_types::trust::content_digest(dir)?;
    Ok(key.signed(&digest, &signature))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that change the process environment; the test
    /// harness runs tests on parallel threads that share it.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A minimal valid package for `app_id` in `src` declaring `capabilities`.
    fn write_package(src: &Path, app_id: &str, capabilities: &str) {
        std::fs::create_dir_all(src.join("ui")).unwrap();
        std::fs::write(src.join("app.wasm"), b"\0asm").unwrap();
        std::fs::write(src.join("ui/index.html"), b"").unwrap();
        std::fs::write(
            src.join("wapp.toml"),
            format!(
                "[package]\nid = \"{app_id}\"\nname = \"D\"\nversion = \"1.0.0\"\n\
                 capabilities = [{capabilities}]\n\n\
                 [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n"
            ),
        )
        .unwrap();
    }

    /// Runs `f` with HOME set to `home` and XDG_DATA_HOME to `home/share`.
    fn with_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let _env = env_lock();
        let prior: Vec<_> = ["HOME", "XDG_DATA_HOME"]
            .iter()
            .map(|k| (*k, std::env::var_os(k)))
            .collect();
        // SAFETY: env_lock serialises every test that touches the environment.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("XDG_DATA_HOME", home.join("share"));
        }
        let result = f();
        for (key, value) in prior {
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        result
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("weft_pack_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    const APP_DATA: &str = "\"fs:rw:app-data\"";

    /// Runs `f` like `with_home`, with only the keys in `home/keys` trusted.
    fn with_trust<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        with_home(home, || {
            let prior = std::env::var_os("WEFT_TRUSTED_KEYS");
            // SAFETY: with_home holds env_lock.
            unsafe { std::env::set_var("WEFT_TRUSTED_KEYS", home.join("keys")) };
            let result = f();
            unsafe {
                match prior {
                    Some(v) => std::env::set_var("WEFT_TRUSTED_KEYS", v),
                    None => std::env::remove_var("WEFT_TRUSTED_KEYS"),
                }
            }
            result
        })
    }

    /// Writes a signing key derived from `seed` to `home/<name>.key` and
    /// returns its path; with `trusted`, its public key joins `home/keys`.
    fn key(home: &Path, name: &str, seed: u8, trusted: bool) -> PathBuf {
        let signing = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        std::fs::create_dir_all(home.join("keys")).unwrap();
        let path = home.join(format!("{name}.key"));
        std::fs::write(&path, hex::encode(signing.to_bytes())).unwrap();
        if trusted {
            std::fs::write(
                home.join("keys").join(format!("{name}.pub")),
                hex::encode(signing.verifying_key().to_bytes()),
            )
            .unwrap();
        }
        path
    }

    fn owner_of(home: &Path, app_id: &str) -> Option<Owner> {
        weft_ipc_types::trust::read_owner(&weft_ipc_types::trust::owner_record_path(
            &home.join("share"),
            app_id,
        ))
        .unwrap()
    }

    fn staging_left(store: &Path) -> bool {
        std::fs::read_dir(store).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".staging-"))
        })
    }

    #[test]
    fn only_packages_signed_by_a_trusted_key_install_as_verified() {
        let home = temp_root("verified");
        let store = home.join("store");
        let app_id = "org.weft.test.verified";
        write_package(&home.join("src"), app_id, "");
        let publisher = key(&home, "publisher", 1, true);
        let stranger = key(&home, "stranger", 2, false);

        // Unsigned, then signed by a key the host does not trust.
        let unsigned = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        });
        sign_package(&home.join("src"), &stranger).unwrap();
        let untrusted = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        });
        for refused in [&unsigned, &untrusted] {
            let message = format!("{:#}", refused.as_ref().unwrap_err());
            assert!(message.contains("not signed by a trusted key"), "{message}");
        }
        assert!(!store.join(app_id).exists());
        assert!(!staging_left(&store));
        assert_eq!(owner_of(&home, app_id), None);

        // Signed by a trusted key.
        sign_package(&home.join("src"), &publisher).unwrap();
        with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        })
        .unwrap();
        assert!(store.join(app_id).join("signature.sig").exists());
        let key = PublisherKey::from_file(&home.join("keys/publisher.pub")).unwrap();
        assert_eq!(owner_of(&home, app_id), Some(Owner::Verified(key)));
        assert!(!staging_left(&store));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn content_changed_after_signing_is_refused() {
        let home = temp_root("tampered");
        let store = home.join("store");
        let app_id = "org.weft.test.tampered";
        write_package(&home.join("src"), app_id, "");
        sign_package(&home.join("src"), &key(&home, "publisher", 3, true)).unwrap();
        std::fs::write(home.join("src/ui/index.html"), "<script>changed</script>").unwrap();
        let refused = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        });
        assert!(refused.is_err());
        assert!(!store.join(app_id).exists());
        assert!(!staging_left(&store));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_app_id_stays_with_its_first_owner_while_its_data_remains() {
        let home = temp_root("owner");
        let store = home.join("store");
        let app_id = "org.weft.test.owned";
        write_package(&home.join("src"), app_id, APP_DATA);
        sign_package(&home.join("src"), &key(&home, "first", 4, true)).unwrap();
        with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        })
        .unwrap();
        let data = weft_ipc_types::package::app_data_dir(&home.join("share"), app_id);
        std::fs::create_dir_all(&data).unwrap();
        with_trust(&home, || uninstall_package_from(app_id, &store)).unwrap();

        // Another trusted publisher, and a development build, cannot take the
        // ID over while the first owner's data remains.
        sign_package(&home.join("src"), &key(&home, "second", 5, true)).unwrap();
        let other_publisher = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        });
        let development = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        });
        for refused in [&other_publisher, &development] {
            let message = format!("{:#}", refused.as_ref().unwrap_err());
            assert!(message.contains("belongs to publisher"), "{message}");
        }
        assert!(!store.join(app_id).exists());
        assert!(!staging_left(&store));

        // With no data left, uninstall frees the ID.
        std::fs::remove_dir_all(&data).unwrap();
        sign_package(&home.join("src"), &home.join("first.key")).unwrap();
        with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        })
        .unwrap();
        with_trust(&home, || uninstall_package_from(app_id, &store)).unwrap();
        assert_eq!(owner_of(&home, app_id), None);
        with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        })
        .unwrap();
        assert_eq!(owner_of(&home, app_id), Some(Owner::Development));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn packages_with_links_are_refused() {
        let home = temp_root("links");
        let store = home.join("store");
        let app_id = "org.weft.test.links";
        write_package(&home.join("src"), app_id, "");
        std::os::unix::fs::symlink("/etc/hostname", home.join("src/ui/host.txt")).unwrap();
        assert!(check_package(&home.join("src")).is_err());
        let refused = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        });
        assert!(refused.is_err());
        assert!(!store.join(app_id).exists());
        assert!(sign_package(&home.join("src"), &key(&home, "publisher", 6, true)).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_development_id_does_not_become_verified() {
        let home = temp_root("dev_owned");
        let store = home.join("store");
        let app_id = "org.weft.test.devowned";
        write_package(&home.join("src"), app_id, APP_DATA);
        with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        })
        .unwrap();
        let data = weft_ipc_types::package::app_data_dir(&home.join("share"), app_id);
        std::fs::create_dir_all(&data).unwrap();
        with_trust(&home, || uninstall_package_from(app_id, &store)).unwrap();

        sign_package(&home.join("src"), &key(&home, "publisher", 7, true)).unwrap();
        let refused = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Verified)
        });
        let message = format!("{:#}", refused.unwrap_err());
        assert!(message.contains("belongs to development"), "{message}");
        assert!(!store.join(app_id).exists());
        assert_eq!(owner_of(&home, app_id), Some(Owner::Development));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_failed_placement_leaves_no_claim_on_the_id() {
        let home = temp_root("placement");
        let staging = home.join(".staging-x");
        let dest = home.join("org.weft.test.placed");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(dest.join("ui")).unwrap();
        let record = home.join("owners/org.weft.test.placed");
        weft_ipc_types::trust::write_owner(&record, Owner::Development).unwrap();

        let admission = Admission {
            owner: Owner::Development,
            created_record: Some(record.clone()),
        };
        assert!(place(&staging, &dest, admission).is_err());
        assert!(!staging.exists());
        assert!(!record.exists());
        assert!(
            dest.join("ui").is_dir(),
            "the existing package is untouched"
        );

        // A record that existed before this installation is kept.
        std::fs::create_dir_all(&staging).unwrap();
        weft_ipc_types::trust::write_owner(&record, Owner::Development).unwrap();
        let admission = Admission {
            owner: Owner::Development,
            created_record: None,
        };
        assert!(place(&staging, &dest, admission).is_err());
        assert!(record.exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn installed_files_are_not_writable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_root("modes");
        let store = home.join("store");
        let app_id = "org.weft.test.modes";
        write_package(&home.join("src"), app_id, "");
        let set = |rel: &str, mode: u32| {
            std::fs::set_permissions(
                home.join("src").join(rel),
                std::fs::Permissions::from_mode(mode),
            )
            .unwrap()
        };
        set("ui/index.html", 0o666);
        set("app.wasm", 0o777);
        set("ui", 0o777);
        with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        })
        .unwrap();
        let mode = |rel: &str| {
            std::fs::metadata(store.join(app_id).join(rel))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("ui/index.html"), 0o644);
        assert_eq!(mode("app.wasm"), 0o755);
        assert_eq!(mode("ui"), 0o755);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn packages_with_non_utf8_names_are_refused() {
        use std::os::unix::ffi::OsStrExt;
        let home = temp_root("non_utf8");
        let store = home.join("store");
        let app_id = "org.weft.test.nonutf8";
        write_package(&home.join("src"), app_id, "");
        let name = std::ffi::OsStr::from_bytes(b"ui/\xff.html");
        std::fs::write(home.join("src").join(name), b"").unwrap();
        let refused = with_trust(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        });
        assert!(refused.is_err());
        assert!(!store.join(app_id).exists());
        assert!(!staging_left(&store));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstall_releases_a_record_left_by_an_interrupted_install() {
        let home = temp_root("interrupted");
        let store = home.join("store");
        let app_id = "org.weft.test.interrupted";
        let record = weft_ipc_types::trust::owner_record_path(&home.join("share"), app_id);
        weft_ipc_types::trust::write_owner(&record, Owner::Development).unwrap();
        let data = weft_ipc_types::package::app_data_dir(&home.join("share"), app_id);
        std::fs::create_dir_all(&data).unwrap();

        // While data remains the record stays, and uninstall reports nothing
        // installed.
        assert!(with_home(&home, || uninstall_package_from(app_id, &store)).is_err());
        assert!(record.exists());
        std::fs::remove_dir_all(&data).unwrap();
        with_home(&home, || uninstall_package_from(app_id, &store)).unwrap();
        assert!(!record.exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_record_stays_while_another_store_holds_the_package() {
        let home = temp_root("other_store");
        let app_id = "org.weft.test.otherstore";
        write_package(&home.join("src"), app_id, "");
        let user_store = home.join(".local/share/weft/apps");
        let elsewhere = home.join("elsewhere");
        with_home(&home, || {
            install_package_to(&home.join("src"), &user_store, InstallMode::Development)
        })
        .unwrap();
        assert!(with_home(&home, || uninstall_package_from(app_id, &elsewhere)).is_err());
        assert_eq!(owner_of(&home, app_id), Some(Owner::Development));
        with_home(&home, || uninstall_package_from(app_id, &user_store)).unwrap();
        assert_eq!(owner_of(&home, app_id), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstalling_one_of_two_copies_keeps_the_record() {
        let home = temp_root("two_copies");
        let app_id = "org.weft.test.twocopies";
        write_package(&home.join("src"), app_id, "");
        let user_store = home.join(".local/share/weft/apps");
        let other = home.join("other");
        for store in [&user_store, &other] {
            with_home(&home, || {
                install_package_to(&home.join("src"), store, InstallMode::Development)
            })
            .unwrap();
        }
        with_home(&home, || uninstall_package_from(app_id, &other)).unwrap();
        assert_eq!(owner_of(&home, app_id), Some(Owner::Development));

        // A leftover directory that is not a package does not hold the ID.
        std::fs::remove_dir_all(user_store.join(app_id)).unwrap();
        std::fs::create_dir_all(user_store.join(app_id)).unwrap();
        with_home(&home, || {
            install_package_to(&home.join("src"), &other, InstallMode::Development)
        })
        .unwrap();
        with_home(&home, || uninstall_package_from(app_id, &other)).unwrap();
        assert_eq!(owner_of(&home, app_id), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn copies_never_follow_links_swapped_in() {
        use rustix::fs::{CWD, Mode, OFlags};
        let home = temp_root("swapped");
        for dir in ["outside", "pkg/dir", "held"] {
            std::fs::create_dir_all(home.join(dir)).unwrap();
        }
        std::fs::write(home.join("outside/other"), b"outside").unwrap();
        std::fs::write(home.join("pkg/dir/secret"), b"listed").unwrap();
        let open_dir = |path: &Path| {
            rustix::fs::openat(
                CWD,
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .unwrap()
        };

        // A directory replaced by a link after it was opened, before its
        // entries are read: the copy reads the directory that was opened.
        let dir = open_dir(&home.join("pkg/dir"));
        std::fs::rename(home.join("pkg/dir"), home.join("held/dir")).unwrap();
        std::os::unix::fs::symlink(home.join("outside"), home.join("pkg/dir")).unwrap();
        copy_tree(&dir, &home.join("pkg/dir"), &home.join("copied")).unwrap();
        assert_eq!(
            std::fs::read(home.join("copied/secret")).unwrap(),
            b"listed"
        );
        assert!(!home.join("copied/other").exists());

        // A link where an entry is listed is refused, and so is a FIFO,
        // without waiting for a writer.
        let pkg = open_dir(&home.join("pkg"));
        assert!(copy_tree(&pkg, &home.join("pkg"), &home.join("copied_link")).is_err());
        std::fs::remove_file(home.join("pkg/dir")).unwrap();
        rustix::fs::mknodat(
            CWD,
            home.join("pkg/fifo"),
            rustix::fs::FileType::Fifo,
            Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let pkg = open_dir(&home.join("pkg"));
        assert!(copy_tree(&pkg, &home.join("pkg"), &home.join("copied_fifo")).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstall_keeps_app_data_from_the_user_store() {
        let home = temp_root("keep");
        let app_id = "com.example.keep";
        write_package(&home.join("src"), app_id, APP_DATA);
        let store = home.join(".local/share/weft/apps");
        with_home(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        })
        .unwrap();
        let legacy = store.join(app_id).join("data");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("notes.txt"), "first\nsecond").unwrap();

        with_home(&home, || uninstall_package_from(app_id, &store)).unwrap();
        assert!(!store.join(app_id).exists());
        let kept = weft_ipc_types::package::app_data_dir(&home.join("share"), app_id);
        assert_eq!(
            std::fs::read_to_string(kept.join("notes.txt")).unwrap(),
            "first\nsecond"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&kept).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }

        // A second copy cannot be merged: uninstall refuses and removes nothing.
        with_home(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        })
        .unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        assert!(with_home(&home, || uninstall_package_from(app_id, &store)).is_err());
        assert!(legacy.is_dir() && kept.is_dir());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn data_that_is_not_moved_is_never_removed() {
        let home = temp_root("refuse");
        let app_id = "com.example.refuse";
        // No app-data capability: the data directory is left and uninstall stops.
        write_package(&home.join("src"), app_id, "");
        let store = home.join(".local/share/weft/apps");
        with_home(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        })
        .unwrap();
        std::fs::create_dir_all(store.join(app_id).join("data")).unwrap();
        std::fs::write(store.join(app_id).join("data/x"), "x").unwrap();
        assert!(with_home(&home, || uninstall_package_from(app_id, &store)).is_err());
        assert_eq!(
            std::fs::read_to_string(store.join(app_id).join("data/x")).unwrap(),
            "x"
        );
        std::fs::remove_dir_all(store.join(app_id)).unwrap();

        // Another store: earlier versions never kept data there, so it stops too.
        write_package(&home.join("src2"), app_id, APP_DATA);
        let other = home.join("other-store");
        with_home(&home, || {
            install_package_to(&home.join("src2"), &other, InstallMode::Development)
        })
        .unwrap();
        std::fs::create_dir_all(other.join(app_id).join("data")).unwrap();
        assert!(with_home(&home, || uninstall_package_from(app_id, &other)).is_err());
        assert!(other.join(app_id).join("data").is_dir());
        assert!(!home.join("share/weft/app-data").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn data_left_without_a_manifest_is_moved_not_deleted() {
        let home = temp_root("orphan");
        let app_id = "org.weft.demo.notes";
        let store = home.join(".local/share/weft/apps");
        std::fs::create_dir_all(store.join(app_id).join("data")).unwrap();
        std::fs::write(store.join(app_id).join("data/notes.txt"), "kept").unwrap();
        // The store reached through another spelling of the same path.
        let spelled = home.join(".local/share/weft/../weft/apps");
        with_home(&home, || uninstall_package_from(app_id, &spelled)).unwrap();
        let kept = weft_ipc_types::package::app_data_dir(&home.join("share"), app_id);
        assert_eq!(
            std::fs::read_to_string(kept.join("notes.txt")).unwrap(),
            "kept"
        );
        assert!(!store.join(app_id).exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_home_still_finds_the_user_store() {
        let root = temp_root("linked");
        let real = root.join("var-home");
        let app_id = "org.weft.demo.notes";
        std::fs::create_dir_all(
            real.join(".local/share/weft/apps")
                .join(app_id)
                .join("data"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&real, root.join("home")).unwrap();
        let store = real.join(".local/share/weft/apps");
        with_home(&root.join("home"), || {
            uninstall_package_from(app_id, &store)
        })
        .unwrap();
        assert!(weft_ipc_types::package::app_data_dir(&root.join("home/share"), app_id).is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn packages_cannot_ship_a_top_level_data_entry() {
        let home = temp_root("packages_cannot_ship_a_top_level_data_entry");
        with_home(&home.clone(), move || {
            let root = temp_root("ships-data");
            let src = root.join("src");
            write_package(&src, "com.example.ships", "");
            std::fs::create_dir_all(src.join("data")).unwrap();
            std::fs::write(src.join("data/asset.json"), "{}").unwrap();
            let err = check_package(&src).unwrap_err().to_string();
            assert!(err.contains("'data'"), "{err}");
            assert!(
                install_package_to(&src, &root.join("store"), InstallMode::Development).is_err()
            );
            let _ = std::fs::remove_dir_all(&root);
        });
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn install_moves_leftover_app_data_out_of_the_way() {
        let home = temp_root("leftover");
        let app_id = "com.example.leftover";
        write_package(&home.join("src"), app_id, APP_DATA);
        let store = home.join(".local/share/weft/apps");
        std::fs::create_dir_all(store.join(app_id).join("data")).unwrap();
        std::fs::write(store.join(app_id).join("data/x"), "x").unwrap();

        // The moved data has no recorded owner, so it is given to this
        // package only when asked.
        let unclaimed = with_home(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        });
        let message = format!("{:#}", unclaimed.unwrap_err());
        assert!(message.contains("--claim-data"), "{message}");
        assert!(!store.join(app_id).join("wapp.toml").exists());
        with_home(&home, || {
            install_into(&home.join("src"), &store, InstallMode::Development, true)
        })
        .unwrap();
        assert!(store.join(app_id).join("wapp.toml").exists());
        assert!(!store.join(app_id).join("data").exists());
        let kept = weft_ipc_types::package::app_data_dir(&home.join("share"), app_id);
        assert_eq!(std::fs::read_to_string(kept.join("x")).unwrap(), "x");

        // An empty directory left behind does not block installation either.
        with_home(&home, || uninstall_package_from(app_id, &store)).unwrap();
        std::fs::create_dir_all(store.join(app_id)).unwrap();
        with_home(&home, || {
            install_package_to(&home.join("src"), &store, InstallMode::Development)
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn app_id_valid() {
        assert!(weft_ipc_types::package::is_valid_app_id(
            "com.example.notes"
        ));
        assert!(weft_ipc_types::package::is_valid_app_id(
            "org.weft.calculator"
        ));
        assert!(weft_ipc_types::package::is_valid_app_id(
            "io.github.username.app"
        ));
    }

    #[test]
    fn app_id_invalid() {
        assert!(!weft_ipc_types::package::is_valid_app_id("com.example"));
        assert!(!weft_ipc_types::package::is_valid_app_id(
            "Com.example.notes"
        ));
        assert!(!weft_ipc_types::package::is_valid_app_id(
            "com.example.notes-app"
        ));
        assert!(!weft_ipc_types::package::is_valid_app_id(
            "com..example.notes"
        ));
        assert!(!weft_ipc_types::package::is_valid_app_id(""));
        assert!(!weft_ipc_types::package::is_valid_app_id(
            "com.Example.notes"
        ));
    }

    #[test]
    fn check_package_missing_manifest() {
        let tmp = std::env::temp_dir().join("weft_pack_test_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let result = check_package(&tmp);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_package_missing_wasm() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_no_wasm");
        let ui_dir = tmp.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(ui_dir.join("index.html"), b"").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "com.example.nowasm"
name = "No Wasm"
version = "0.1.0"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
"#,
        )
        .unwrap();
        let result = check_package(&tmp);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_package_missing_ui_entry() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_no_ui");
        let _ = fs::create_dir_all(&tmp);
        fs::write(tmp.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "com.example.noui"
name = "No UI"
version = "0.1.0"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
"#,
        )
        .unwrap();
        let result = check_package(&tmp);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_package_valid() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_valid");
        let ui_dir = tmp.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(tmp.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(ui_dir.join("index.html"), b"<!DOCTYPE html>").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "com.example.test"
name = "Test App"
version = "1.0.0"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
"#,
        )
        .unwrap();

        let result = check_package(&tmp);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(result.unwrap(), "OK");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn install_package_copies_to_store() {
        let home = temp_root("install_package_copies_to_store");
        with_home(&home.clone(), move || {
            use std::fs;
            let id = format!("weft.pack.install{}", std::process::id());
            let src = std::env::temp_dir().join(format!("weft_pack_install_src_{}", id));
            let store = std::env::temp_dir().join(format!("weft_pack_install_store_{}", id));
            let ui_dir = src.join("ui");
            let _ = fs::create_dir_all(&ui_dir);
            fs::write(src.join("app.wasm"), b"\0asm").unwrap();
            fs::write(ui_dir.join("index.html"), b"<!DOCTYPE html>").unwrap();
            let app_id = format!("com.example.t{}", std::process::id());
            fs::write(
                src.join("wapp.toml"),
                format!(
                    "[package]\nid = \"{app_id}\"\nname = \"Test\"\nversion = \"1.0.0\"\n\n\
                     [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n"
                ),
            )
            .unwrap();
            let result = install_package_to(&src, &store, InstallMode::Development);
            assert!(result.is_ok(), "{result:?}");
            assert!(store.join(&app_id).join("app.wasm").exists());
            assert!(store.join(&app_id).join("wapp.toml").exists());
            assert!(store.join(&app_id).join("ui").join("index.html").exists());
            assert!(install_package_to(&src, &store, InstallMode::Development).is_err());
            let _ = fs::remove_dir_all(&src);
            let _ = fs::remove_dir_all(&store);
        });
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstall_package_removes_directory() {
        let home = temp_root("uninstall_package_removes_directory");
        with_home(&home.clone(), move || {
            use std::fs;
            let id = format!("weft.pack.uninstall{}", std::process::id());
            let src = std::env::temp_dir().join(format!("weft_pack_uninstall_src_{}", id));
            let store = std::env::temp_dir().join(format!("weft_pack_uninstall_store_{}", id));
            let ui_dir = src.join("ui");
            let _ = fs::create_dir_all(&ui_dir);
            fs::write(src.join("app.wasm"), b"\0asm").unwrap();
            fs::write(ui_dir.join("index.html"), b"").unwrap();
            let app_id = format!("com.example.u{}", std::process::id());
            fs::write(
                src.join("wapp.toml"),
                format!(
                    "[package]\nid = \"{app_id}\"\nname = \"U\"\nversion = \"1.0.0\"\n\n\
                     [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n"
                ),
            )
            .unwrap();
            install_package_to(&src, &store, InstallMode::Development).unwrap();
            assert!(store.join(&app_id).exists());
            let result = uninstall_package_from(&app_id, &store);
            assert!(result.is_ok(), "{result:?}");
            assert!(!store.join(&app_id).exists());
            assert!(uninstall_package_from(&app_id, &store).is_err());
            let _ = fs::remove_dir_all(&src);
            let _ = fs::remove_dir_all(&store);
        });
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn check_package_bad_wasm_magic() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_bad_wasm");
        let ui_dir = tmp.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(tmp.join("app.wasm"), b"NOT_WASM").unwrap();
        fs::write(ui_dir.join("index.html"), b"").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "com.example.badwasm"
name = "Bad Wasm"
version = "0.1.0"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
"#,
        )
        .unwrap();
        let result = check_package(&tmp);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("bad magic bytes"), "got: {msg}");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_package_invalid_app_id() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_invalid_id");
        let ui_dir = tmp.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(tmp.join("app.wasm"), b"\0asm").unwrap();
        fs::write(ui_dir.join("index.html"), b"").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "bad-id"
name = "Bad"
version = "0.1.0"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
"#,
        )
        .unwrap();

        let result = check_package(&tmp);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn install_package_from_archive() {
        let home = temp_root("install_package_from_archive");
        with_home(&home.clone(), move || {
            use std::fs;
            let id = format!("instarch_{}", std::process::id());
            let src = std::env::temp_dir().join(&id);
            let ui = src.join("ui");
            let _ = fs::create_dir_all(&ui);
            fs::write(src.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
            fs::write(ui.join("index.html"), b"<!DOCTYPE html>").unwrap();
            let app_id = format!("com.example.ia{}", std::process::id());
            fs::write(
                src.join("wapp.toml"),
                format!(
                    "[package]\nid = \"{app_id}\"\nname = \"IA\"\nversion = \"1.0.0\"\n\n\
                     [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n"
                ),
            )
            .unwrap();

            let bundle_dir = std::env::temp_dir().join(format!("{id}_bnd"));
            let _ = fs::create_dir_all(&bundle_dir);
            bundle_package(&src, Some(&bundle_dir)).unwrap();
            let archive = bundle_dir.join(format!("{app_id}.app.tar.zst"));

            let store = std::env::temp_dir().join(format!("{id}_store"));
            let _ = fs::create_dir_all(&store);
            install_package_to(&archive, &store, InstallMode::Development).unwrap();
            assert!(store.join(&app_id).join("app.wasm").exists());
            let scratch_left = fs::read_dir(&store).unwrap().flatten().any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with(".unpack") || name.starts_with(".staging-")
            });
            assert!(!scratch_left, "archive install left scratch directories");

            let _ = fs::remove_dir_all(&src);
            let _ = fs::remove_dir_all(&bundle_dir);
            let _ = fs::remove_dir_all(&store);
        });
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn bundle_and_unbundle_roundtrip() {
        use std::fs;
        let id = format!("bundle_{}", std::process::id());
        let src = std::env::temp_dir().join(&id);
        let out = std::env::temp_dir().join(format!("{id}_out"));
        let ui = src.join("ui");
        let _ = fs::create_dir_all(&ui);
        fs::write(src.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(ui.join("index.html"), b"<!DOCTYPE html>").unwrap();
        let app_id = format!("com.example.b{}", std::process::id());
        fs::write(
            src.join("wapp.toml"),
            format!(
                "[package]\nid = \"{app_id}\"\nname = \"B\"\nversion = \"1.0.0\"\n\n\
                 [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n"
            ),
        )
        .unwrap();

        let _ = fs::create_dir_all(&out);
        bundle_package(&src, Some(&out)).unwrap();
        let archive = out.join(format!("{app_id}.app.tar.zst"));
        assert!(archive.exists());

        let unpack = std::env::temp_dir().join(format!("{id}_unpack"));
        let _ = fs::create_dir_all(&unpack);
        unbundle_package(&archive, &unpack).unwrap();
        assert!(unpack.join(&app_id).join("app.wasm").exists());
        assert!(unpack.join(&app_id).join("ui").join("index.html").exists());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&out);
        let _ = fs::remove_dir_all(&unpack);
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        use std::fs;
        let id = format!("sign_verify_{}", std::process::id());
        let dir = std::env::temp_dir().join(&id);
        let key_dir = std::env::temp_dir().join(format!("{id}_keys"));
        let ui = dir.join("ui");
        let _ = fs::create_dir_all(&ui);
        fs::write(dir.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(ui.join("index.html"), b"<!DOCTYPE html>").unwrap();
        fs::write(
            dir.join("wapp.toml"),
            "[package]\nid = \"com.example.signed\"\nname = \"S\"\nversion = \"1.0.0\"\n\n\
             [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n",
        )
        .unwrap();

        generate_key(&key_dir).unwrap();
        let key_file = key_dir.join("weft-sign.key");
        let pub_file = key_dir.join("weft-sign.pub");

        sign_package(&dir, &key_file).unwrap();
        assert!(dir.join("signature.sig").exists());

        let ok = verify_package(&dir, &PublisherKey::from_file(&pub_file).unwrap()).unwrap();
        assert!(ok, "signature should verify");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&key_dir);
    }

    #[test]
    fn verify_rejects_tampered_bundle() {
        use std::fs;
        let id = format!("tamper_{}", std::process::id());
        let dir = std::env::temp_dir().join(&id);
        let key_dir = std::env::temp_dir().join(format!("{id}_keys"));
        let ui = dir.join("ui");
        let _ = fs::create_dir_all(&ui);
        fs::write(dir.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(ui.join("index.html"), b"<!DOCTYPE html>").unwrap();
        fs::write(
            dir.join("wapp.toml"),
            "[package]\nid = \"com.example.tamper\"\nname = \"T\"\nversion = \"1.0.0\"\n\n\
             [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n",
        )
        .unwrap();

        generate_key(&key_dir).unwrap();
        sign_package(&dir, &key_dir.join("weft-sign.key")).unwrap();

        fs::write(dir.join("app.wasm"), b"\0asm\x01\0\0\x01").unwrap();

        let ok = verify_package(
            &dir,
            &PublisherKey::from_file(&key_dir.join("weft-sign.pub")).unwrap(),
        )
        .unwrap();
        assert!(!ok, "tampered bundle should not verify");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&key_dir);
    }

    #[test]
    fn list_installed_roots_uses_weft_app_store_when_set() {
        let _env = env_lock();
        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::set_var("WEFT_APP_STORE", "/custom/store") };
        let roots = list_installed_roots();
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
        }
        assert_eq!(roots, vec![PathBuf::from("/custom/store")]);
    }

    #[test]
    fn list_installed_roots_includes_system_path() {
        let _env = env_lock();
        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::remove_var("WEFT_APP_STORE") };
        let roots = list_installed_roots();
        unsafe {
            if let Some(v) = prior {
                std::env::set_var("WEFT_APP_STORE", v);
            }
        }
        assert!(
            roots
                .iter()
                .any(|p| p == &PathBuf::from("/usr/share/weft/apps"))
        );
    }

    fn make_valid_package(dir: &std::path::Path, id: &str, caps: &str) {
        use std::fs;
        let ui = dir.join("ui");
        let _ = fs::create_dir_all(&ui);
        fs::write(dir.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(ui.join("index.html"), b"").unwrap();
        fs::write(
            dir.join("wapp.toml"),
            format!(
                "[package]\nid = \"{id}\"\nname = \"T\"\nversion = \"0.1.0\"\n{caps}\n\
                 [runtime]\nmodule = \"app.wasm\"\n[ui]\nentry = \"ui/index.html\"\n"
            ),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn check_package_refuses_entries_outside_the_package() {
        let tmp = std::env::temp_dir().join(format!("weft_pack_entries_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        make_valid_package(&tmp, "com.example.entries", "");
        std::fs::write(tmp.with_extension("wasm"), b"\0asm\x01\0\0\0").unwrap();
        std::os::unix::fs::symlink("index.html", tmp.join("ui/linked.html")).unwrap();
        let manifest = std::fs::read_to_string(tmp.join("wapp.toml")).unwrap();
        let escaping = format!(
            "../{}",
            tmp.with_extension("wasm").file_name().unwrap().display()
        );
        std::fs::write(
            tmp.join("wapp.toml"),
            manifest
                .replace("module = \"app.wasm\"", &format!("module = \"{escaping}\""))
                .replace("ui/index.html", "ui/linked.html"),
        )
        .unwrap();
        let msg = check_package(&tmp).unwrap_err().to_string();
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_file(tmp.with_extension("wasm"));
        assert!(
            msg.contains("runtime.module") && msg.contains("not a relative path"),
            "{msg}"
        );
        assert!(
            msg.contains("ui.entry") && msg.contains("symbolic link"),
            "{msg}"
        );
    }

    #[test]
    fn check_package_unknown_capability_errors() {
        let tmp = std::env::temp_dir().join(format!("weft_pack_caps_bad_{}", std::process::id()));
        make_valid_package(&tmp, "com.example.caps", "capabilities = [\"network:tcp\"]");
        let result = check_package(&tmp);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("network:tcp"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_package_known_capabilities_accepted() {
        let tmp = std::env::temp_dir().join(format!("weft_pack_caps_ok_{}", std::process::id()));
        make_valid_package(
            &tmp,
            "com.example.caps",
            "capabilities = [\"fs:rw:app-data\", \"fs:read:xdg-documents\"]",
        );
        assert!(check_package(&tmp).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
