//! Effective grants for a session, derived once from the package manifest.
//!
//! Every capability the package declares must be understood and satisfiable
//! by this host, or the session does not start. The result is passed to
//! weft-runtime explicitly (`--preopen`, `--grant`) and to the file portal;
//! neither reads the manifest itself.

use std::path::{Path, PathBuf};

use weft_ipc_types::capability::{Access, Capability};

/// A host directory made visible to the component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantedDir {
    pub host: PathBuf,
    pub guest: &'static str,
    pub access: Access,
}

impl GrantedDir {
    /// The `--preopen` argument for weft-runtime.
    pub fn preopen_arg(&self) -> String {
        let mode = match self.access {
            Access::Read => "ro",
            Access::ReadWrite => "rw",
        };
        format!("{}::{}::{mode}", self.host.display(), self.guest)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionGrants {
    pub dirs: Vec<GrantedDir>,
    /// Host-import capabilities, passed as `--grant`.
    pub imports: Vec<Capability>,
}

/// Why a launch is refused, with the error code reported to the client:
/// 400 for a malformed app ID, 404 for a package that is not installed, 403
/// for a package this host will not run as declared and 500 for a host
/// fault.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub code: u32,
    pub message: String,
}

impl Refusal {
    fn new(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Where the host resources behind filesystem capabilities live.
pub(crate) struct HostDirs {
    /// The user's data home (`$XDG_DATA_HOME`); app data lives under it in
    /// `weft/app-data/<id>`, apart from installed packages.
    pub data_home: PathBuf,
    /// The home directory, under which earlier versions kept app data.
    pub home: PathBuf,
    /// The user's documents directory, if the platform configures one.
    pub documents: Option<PathBuf>,
}

impl HostDirs {
    pub fn from_env() -> Result<Self, Refusal> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or_else(|| Refusal::new(500, "HOME is not set to an absolute path"))?;
        // The XDG base directory specification ignores relative values.
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        let data_home = weft_ipc_types::package::data_home()
            .ok_or_else(|| Refusal::new(500, "no data home: HOME is not set"))?;
        Ok(Self {
            data_home,
            documents: documents_dir(&home, &config),
            home,
        })
    }
}

/// Reads the package manifest of `app_id` and derives its grants.
pub(crate) fn for_app(app_id: &str) -> Result<SessionGrants, Refusal> {
    #[derive(serde::Deserialize)]
    struct Package {
        capabilities: Option<Vec<String>>,
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        package: Package,
    }

    if !weft_ipc_types::package::is_valid_app_id(app_id) {
        return Err(Refusal::new(400, "invalid app ID"));
    }
    let manifest = crate::app_store_roots()
        .into_iter()
        .map(|root| root.join(app_id).join("wapp.toml"))
        .find(|path| path.exists())
        .ok_or_else(|| Refusal::new(404, format!("package {app_id} is not installed")))?;
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| Refusal::new(500, format!("cannot read {}: {e}", manifest.display())))?;
    let manifest: Manifest = toml::from_str(&text)
        .map_err(|e| Refusal::new(403, format!("invalid {}: {e}", manifest.display())))?;
    let declared = manifest.package.capabilities.unwrap_or_default();
    derive(app_id, &declared, HostDirs::from_env)
}

/// Derives grants for the declared capabilities. Every capability is
/// checked before host directories are resolved or the app's data
/// directory is created, so a refused launch leaves nothing behind.
pub(crate) fn derive(
    app_id: &str,
    declared: &[String],
    host_dirs: impl FnOnce() -> Result<HostDirs, Refusal>,
) -> Result<SessionGrants, Refusal> {
    let mut capabilities = Vec::new();
    for text in declared {
        let capability: Capability =
            text.parse()
                .map_err(|e: weft_ipc_types::capability::UnknownCapability| {
                    Refusal::new(403, e.to_string())
                })?;
        if matches!(capability, Capability::GpuCompute | Capability::GpuRender) {
            return Err(Refusal::new(
                403,
                format!("{capability} is not supported by this host"),
            ));
        }
        capabilities.push(capability);
    }

    let mut grants = SessionGrants::default();
    let needs_dirs = capabilities
        .iter()
        .any(|c| matches!(c, Capability::AppData(_) | Capability::Documents(_)));
    let host = if needs_dirs { Some(host_dirs()?) } else { None };
    for capability in capabilities {
        match (&capability, &host) {
            (Capability::AppData(access), Some(host)) => {
                let dir = weft_ipc_types::package::app_data_dir(&host.data_home, app_id);
                grant_dir(&mut grants, dir, "/data", *access)?;
            }
            (Capability::Documents(access), Some(host)) => {
                let dir = host.documents.clone().ok_or_else(|| {
                    Refusal::new(
                        403,
                        format!("{capability} requested, but no documents directory is configured"),
                    )
                })?;
                grant_dir(&mut grants, dir, "/xdg/documents", *access)?;
            }
            _ => {
                if !grants.imports.contains(&capability) {
                    grants.imports.push(capability);
                }
            }
        }
    }
    if let (Some(dir), Some(host)) = (grants.dirs.iter().find(|d| d.guest == "/data"), &host) {
        prepare_app_data(app_id, &dir.host, host)?;
    }
    Ok(grants)
}

/// Creates the app's data directory, first moving data an earlier version
/// kept inside the package store. A conflict between the two locations
/// refuses the launch (409) and leaves both untouched.
fn prepare_app_data(app_id: &str, dir: &Path, host: &HostDirs) -> Result<(), Refusal> {
    use weft_ipc_types::package::{Migration, legacy_app_data_dir, migrate_app_data};
    let legacy = legacy_app_data_dir(&host.home, app_id);
    match migrate_app_data(&legacy, dir) {
        Ok(Migration::Moved) => {
            tracing::info!(%app_id, from = %legacy.display(), to = %dir.display(), "app data moved");
        }
        Ok(Migration::NotNeeded) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Refusal::new(409, e.to_string()));
        }
        Err(e) => {
            return Err(Refusal::new(
                500,
                format!("cannot move app data from {}: {e}", legacy.display()),
            ));
        }
    }
    create_private_dir(dir)
        .map_err(|e| Refusal::new(500, format!("cannot create {}: {e}", dir.display())))
}

/// Creates `dir` and missing parents, and makes the directory itself
/// accessible only to the user, including when it already existed or was
/// moved from the earlier layout.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => {}
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Adds a directory grant; declaring both modes for one directory grants the
/// wider one. The host path must survive the `--preopen HOST::GUEST::MODE`
/// encoding unchanged: it must be UTF-8 and must not contain `::`.
fn grant_dir(
    grants: &mut SessionGrants,
    host: PathBuf,
    guest: &'static str,
    access: Access,
) -> Result<(), Refusal> {
    if host.to_str().is_none_or(|path| path.contains("::")) {
        return Err(Refusal::new(
            403,
            format!("{} cannot be granted to a session", host.display()),
        ));
    }
    match grants.dirs.iter_mut().find(|dir| dir.guest == guest) {
        Some(existing) => {
            if access == Access::ReadWrite {
                existing.access = Access::ReadWrite;
            }
        }
        None => grants.dirs.push(GrantedDir {
            host,
            guest,
            access,
        }),
    }
    Ok(())
}

/// The documents directory from the XDG user directories configuration
/// (`user-dirs.dirs`), or None when it is not configured, is disabled (set
/// to the home directory) or does not exist. Values are `"$HOME/path"` or
/// `"/absolute/path"`; the last assignment wins, as when the file is sourced
/// by a shell. Values with escapes are not interpreted and are ignored.
fn documents_dir(home: &Path, config: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    let value = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("XDG_DOCUMENTS_DIR="))
        .next_back()?
        .trim();
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    if value.contains('\\') || value.contains('"') {
        return None;
    }
    let dir = match value.strip_prefix("$HOME") {
        Some("") => home.to_path_buf(),
        Some(rest) => home.join(rest.strip_prefix('/')?),
        None => PathBuf::from(value),
    };
    (dir.is_absolute() && dir != home && dir.is_dir()).then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weft_grants_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn host(root: &Path, documents: Option<PathBuf>) -> impl FnOnce() -> Result<HostDirs, Refusal> {
        let root = root.to_path_buf();
        move || {
            Ok(HostDirs {
                data_home: root.join("share"),
                home: root.join("home"),
                documents,
            })
        }
    }

    fn no_host() -> Result<HostDirs, Refusal> {
        Err(Refusal::new(500, "HOME is not set to an absolute path"))
    }

    fn caps(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_capabilities_grant_nothing_and_need_no_home() {
        let grants = derive("org.example.app", &[], no_host).unwrap();
        assert_eq!(grants, SessionGrants::default());
        let grants = derive("org.example.app", &caps(&["sys:notifications"]), no_host).unwrap();
        assert_eq!(grants.imports, [Capability::Notifications]);
        assert_eq!(
            derive("org.example.app", &caps(&["fs:rw:app-data"]), no_host)
                .unwrap_err()
                .code,
            500
        );
    }

    #[test]
    fn data_access_modes_stay_distinct() {
        let root = temp("modes");
        let read = derive(
            "org.example.app",
            &caps(&["fs:read:app-data"]),
            host(&root, None),
        )
        .unwrap();
        assert_eq!(read.dirs[0].access, Access::Read);
        assert!(read.dirs[0].preopen_arg().ends_with("::/data::ro"));
        assert!(root.join("share/weft/app-data/org.example.app").is_dir());

        let both = derive(
            "org.example.app",
            &caps(&["fs:read:app-data", "fs:rw:app-data"]),
            host(&root, None),
        )
        .unwrap();
        assert_eq!(both.dirs.len(), 1);
        assert_eq!(both.dirs[0].access, Access::ReadWrite);
    }

    #[test]
    fn imports_are_listed_once() {
        let root = temp("imports");
        let grants = derive(
            "org.example.app",
            &caps(&[
                "sys:notifications",
                "net:fetch:api.example.org",
                "sys:notifications",
            ]),
            host(&root, None),
        )
        .unwrap();
        let listed: Vec<String> = grants.imports.iter().map(|c| c.to_string()).collect();
        assert_eq!(listed, ["sys:notifications", "net:fetch:api.example.org"]);
        assert!(grants.dirs.is_empty());
    }

    #[test]
    fn refused_launches_create_nothing() {
        let root = temp("fail");
        for declared in [
            &["sys:everything"][..],
            &["hw:gpu:compute"],
            &["fs:read:xdg-documents"],
            &["fs:rw:app-data", "hw:gpu:render"],
            &["fs:rw:app-data", "fs:read:xdg-documents"],
        ] {
            let refusal =
                derive("org.example.app", &caps(declared), host(&root, None)).unwrap_err();
            assert_eq!(refusal.code, 403, "{declared:?}");
        }
        assert!(!root.join("share").exists());
    }

    #[test]
    fn paths_that_cannot_be_encoded_are_refused() {
        let root = temp("encode");
        let odd = root.join("a::b");
        std::fs::create_dir_all(&odd).unwrap();
        let refusal = derive(
            "org.example.app",
            &caps(&["fs:read:xdg-documents"]),
            host(&root, Some(odd)),
        )
        .unwrap_err();
        assert_eq!(refusal.code, 403);
    }

    #[test]
    fn documents_come_from_user_dirs() {
        let home = temp("home");
        let config = home.join(".config");
        std::fs::create_dir_all(home.join("Docs")).unwrap();
        std::fs::create_dir_all(home.join("Later")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        assert_eq!(documents_dir(&home, &config), None);

        let write = |text: &str| std::fs::write(config.join("user-dirs.dirs"), text).unwrap();
        write("# comment\nXDG_DOCUMENTS_DIR=\"$HOME/Docs\"\n");
        assert_eq!(documents_dir(&home, &config), Some(home.join("Docs")));
        write("XDG_DOCUMENTS_DIR=\"$HOME/Docs\"\nXDG_DOCUMENTS_DIR=\"$HOME/Later\"\n");
        assert_eq!(documents_dir(&home, &config), Some(home.join("Later")));
        let absolute = format!("XDG_DOCUMENTS_DIR=\"{}\"\n", home.join("Docs").display());
        write(&absolute);
        assert_eq!(documents_dir(&home, &config), Some(home.join("Docs")));

        // Disabled, missing, relative, escaped and malformed values.
        for text in [
            "XDG_DOCUMENTS_DIR=\"$HOME/\"\n",
            "XDG_DOCUMENTS_DIR=\"$HOME\"\n",
            "XDG_DOCUMENTS_DIR=\"$HOME/Missing\"\n",
            "XDG_DOCUMENTS_DIR=\"$HOMEDocs\"\n",
            "XDG_DOCUMENTS_DIR=\"Docs\"\n",
            "XDG_DOCUMENTS_DIR=\"$HOME/Do\\\"cs\"\n",
            "XDG_DOCUMENTS_DIR=$HOME/Docs\n",
        ] {
            write(text);
            assert_eq!(documents_dir(&home, &config), None, "{text:?}");
        }

        let grants = derive(
            "org.example.app",
            &caps(&["fs:rw:xdg-documents"]),
            host(&home, Some(home.join("Docs"))),
        )
        .unwrap();
        assert_eq!(grants.dirs[0].guest, "/xdg/documents");
        assert_eq!(grants.dirs[0].access, Access::ReadWrite);
    }

    #[test]
    fn earlier_app_data_is_moved_on_launch() {
        let root = temp("migrate");
        let legacy = root.join("home/.local/share/weft/apps/org.example.app/data");
        std::fs::create_dir_all(&legacy).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(legacy.join("notes.txt"), "kept").unwrap();
        let grants = derive(
            "org.example.app",
            &caps(&["fs:rw:app-data"]),
            host(&root, None),
        )
        .unwrap();
        let data = root.join("share/weft/app-data/org.example.app");
        assert_eq!(grants.dirs[0].host, data);
        assert_eq!(
            std::fs::read_to_string(data.join("notes.txt")).unwrap(),
            "kept"
        );
        assert!(!legacy.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&data).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn conflicting_app_data_refuses_the_launch() {
        let root = temp("conflict");
        let legacy = root.join("home/.local/share/weft/apps/org.example.app/data");
        let data = root.join("share/weft/app-data/org.example.app");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let refusal = derive(
            "org.example.app",
            &caps(&["fs:read:app-data"]),
            host(&root, None),
        )
        .unwrap_err();
        assert_eq!(refusal.code, 409);
        assert!(legacy.is_dir() && data.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn app_data_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("private");
        derive(
            "org.example.app",
            &caps(&["fs:rw:app-data"]),
            host(&root, None),
        )
        .unwrap();
        let mode = std::fs::metadata(root.join("share/weft/app-data/org.example.app"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
