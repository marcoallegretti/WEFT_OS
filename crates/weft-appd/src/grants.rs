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

/// Where the host resources behind filesystem capabilities live.
pub(crate) struct HostDirs {
    /// Root under which each app's private data directory is created.
    pub app_data_root: PathBuf,
    /// The user's documents directory, if the platform configures one.
    pub documents: Option<PathBuf>,
}

impl HostDirs {
    pub fn from_env() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set")?;
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self {
            app_data_root: home.join(".local/share/weft/apps"),
            documents: documents_dir(&home, &config),
        })
    }
}

/// Reads the package manifest of `app_id` and derives its grants.
pub(crate) fn for_app(app_id: &str) -> Result<SessionGrants, String> {
    #[derive(serde::Deserialize)]
    struct Package {
        capabilities: Option<Vec<String>>,
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        package: Package,
    }

    let manifest = crate::app_store_roots()
        .into_iter()
        .map(|root| root.join(app_id).join("wapp.toml"))
        .find(|path| path.exists())
        .ok_or_else(|| format!("package {app_id} is not installed"))?;
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", manifest.display()))?;
    let declared = manifest.package.capabilities.unwrap_or_default();
    derive(app_id, &declared, &HostDirs::from_env()?)
}

/// Derives grants for the declared capabilities. Creates the app's private
/// data directory when a data capability is declared.
pub(crate) fn derive(
    app_id: &str,
    declared: &[String],
    host: &HostDirs,
) -> Result<SessionGrants, String> {
    let mut grants = SessionGrants::default();
    for text in declared {
        let capability: Capability = text.parse().map_err(|e| format!("{e}"))?;
        match capability {
            Capability::AppData(access) => {
                let dir = host.app_data_root.join(app_id).join("data");
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                grant_dir(&mut grants, dir, "/data", access);
            }
            Capability::Documents(access) => {
                let dir = host.documents.clone().ok_or_else(|| {
                    format!("{capability} requested, but no documents directory is configured")
                })?;
                grant_dir(&mut grants, dir, "/xdg/documents", access);
            }
            Capability::Fetch(_)
            | Capability::Notifications
            | Capability::ClipboardRead
            | Capability::ClipboardWrite => {
                if !grants.imports.contains(&capability) {
                    grants.imports.push(capability);
                }
            }
            Capability::GpuCompute | Capability::GpuRender => {
                return Err(format!("{capability} is not supported by this host"));
            }
        }
    }
    Ok(grants)
}

/// Adds a directory grant; declaring both modes for one directory grants the
/// wider one.
fn grant_dir(grants: &mut SessionGrants, host: PathBuf, guest: &'static str, access: Access) {
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
}

/// The documents directory from the XDG user directories configuration
/// (`user-dirs.dirs`), or None when it is not configured, is disabled (set
/// to the home directory) or does not exist.
fn documents_dir(home: &Path, config: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    let value = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("XDG_DOCUMENTS_DIR=")
            .map(|v| v.trim().trim_matches('"').to_owned())
    })?;
    let dir = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(&value),
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

    fn host(root: &Path, documents: Option<PathBuf>) -> HostDirs {
        HostDirs {
            app_data_root: root.join("apps"),
            documents,
        }
    }

    fn caps(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_capabilities_grant_nothing() {
        let root = temp("none");
        let grants = derive("org.example.app", &[], &host(&root, None)).unwrap();
        assert_eq!(grants, SessionGrants::default());
    }

    #[test]
    fn data_access_modes_stay_distinct() {
        let root = temp("modes");
        let read = derive(
            "org.example.app",
            &caps(&["fs:read:app-data"]),
            &host(&root, None),
        )
        .unwrap();
        assert_eq!(read.dirs[0].access, Access::Read);
        assert!(read.dirs[0].preopen_arg().ends_with("::/data::ro"));
        assert!(root.join("apps/org.example.app/data").is_dir());

        let both = derive(
            "org.example.app",
            &caps(&["fs:read:app-data", "fs:rw:app-data"]),
            &host(&root, None),
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
            &host(&root, None),
        )
        .unwrap();
        let listed: Vec<String> = grants.imports.iter().map(|c| c.to_string()).collect();
        assert_eq!(listed, ["sys:notifications", "net:fetch:api.example.org"]);
        assert!(grants.dirs.is_empty());
    }

    #[test]
    fn unknown_unsupported_and_unsatisfiable_capabilities_fail() {
        let root = temp("fail");
        for declared in ["sys:everything", "hw:gpu:compute", "fs:read:xdg-documents"] {
            assert!(
                derive("org.example.app", &caps(&[declared]), &host(&root, None)).is_err(),
                "{declared} accepted"
            );
        }
    }

    #[test]
    fn documents_come_from_user_dirs() {
        let home = temp("home");
        let config = home.join(".config");
        std::fs::create_dir_all(home.join("Docs")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        assert_eq!(documents_dir(&home, &config), None);

        std::fs::write(
            config.join("user-dirs.dirs"),
            "# comment\nXDG_DOCUMENTS_DIR=\"$HOME/Docs\"\n",
        )
        .unwrap();
        assert_eq!(documents_dir(&home, &config), Some(home.join("Docs")));

        // Disabled (the home directory itself) and missing directories.
        std::fs::write(
            config.join("user-dirs.dirs"),
            "XDG_DOCUMENTS_DIR=\"$HOME/\"\n",
        )
        .unwrap();
        assert_eq!(documents_dir(&home, &config), None);
        std::fs::write(
            config.join("user-dirs.dirs"),
            "XDG_DOCUMENTS_DIR=\"$HOME/Missing\"\n",
        )
        .unwrap();
        assert_eq!(documents_dir(&home, &config), None);

        let grants = derive(
            "org.example.app",
            &caps(&["fs:rw:xdg-documents"]),
            &host(&home, Some(home.join("Docs"))),
        )
        .unwrap();
        assert_eq!(grants.dirs[0].guest, "/xdg/documents");
        assert_eq!(grants.dirs[0].access, Access::ReadWrite);
    }
}
