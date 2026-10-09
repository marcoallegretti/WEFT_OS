//! The effective grants weft-appd derived for this session.
//!
//! weft-appd passes each host-import capability as `--grant <capability>` and
//! each filesystem capability as `--preopen HOST::GUEST::ro|rw`. Nothing here
//! reads the package manifest: the runtime enforces what the supervisor
//! granted, not what the package asks for.

use anyhow::Context;
use weft_ipc_types::capability::{Access, Capability, FetchScope};

/// Grants for the host imports in the `weft:app` world.
#[derive(Debug, Default)]
pub struct Grants {
    fetch: Vec<FetchScope>,
    notifications: bool,
    clipboard_read: bool,
    clipboard_write: bool,
}

// The checks are used by the import implementations, which only exist with
// the wasmtime-runtime feature.
#[cfg_attr(not(feature = "wasmtime-runtime"), allow(dead_code))]
impl Grants {
    /// Adds a `--grant` argument. Filesystem capabilities arrive as preopens
    /// and hardware capabilities have no import, so both are refused here.
    pub fn add(&mut self, spec: &str) -> anyhow::Result<()> {
        match spec.parse::<Capability>()? {
            Capability::Fetch(scope) => self.fetch.push(scope),
            Capability::Notifications => self.notifications = true,
            Capability::ClipboardRead => self.clipboard_read = true,
            Capability::ClipboardWrite => self.clipboard_write = true,
            other => anyhow::bail!("'{other}' is not granted through --grant"),
        }
        Ok(())
    }

    pub fn notifications(&self) -> Result<(), String> {
        granted(self.notifications, "sys:notifications")
    }

    pub fn clipboard_read(&self) -> Result<(), String> {
        granted(self.clipboard_read, "sys:clipboard:read")
    }

    pub fn clipboard_write(&self) -> Result<(), String> {
        granted(self.clipboard_write, "sys:clipboard:write")
    }

    #[cfg_attr(not(feature = "net-fetch"), allow(dead_code))]
    /// Whether a request to `host` is covered by a fetch grant. `host` is the
    /// URL's host in the lowercase form URL parsing produces.
    pub fn fetch_host(&self, host: &str) -> Result<(), String> {
        if self.fetch.is_empty() {
            return Err("capability net:fetch is not granted".to_owned());
        }
        let covered = self.fetch.iter().any(|scope| match scope {
            FetchScope::AnyHost => true,
            FetchScope::Host(granted) => granted == host,
        });
        if covered {
            Ok(())
        } else {
            Err(format!("capability net:fetch:{host} is not granted"))
        }
    }

    /// Whether any fetch grant exists, before a URL is examined.
    pub fn fetch_any(&self) -> Result<(), String> {
        granted(!self.fetch.is_empty(), "net:fetch")
    }
}

#[cfg_attr(not(feature = "wasmtime-runtime"), allow(dead_code))]
fn granted(present: bool, capability: &str) -> Result<(), String> {
    if present {
        Ok(())
    } else {
        Err(format!("capability {capability} is not granted"))
    }
}

/// A directory made visible to the component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preopen {
    pub host: String,
    pub guest: String,
    pub access: Access,
}

impl Preopen {
    /// Parses `HOST::GUEST::ro` or `HOST::GUEST::rw`.
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let (rest, mode) = spec
            .rsplit_once("::")
            .context("--preopen expects HOST::GUEST::ro|rw")?;
        let (host, guest) = rest
            .split_once("::")
            .context("--preopen expects HOST::GUEST::ro|rw")?;
        let access = match mode {
            "ro" => Access::Read,
            "rw" => Access::ReadWrite,
            other => anyhow::bail!("--preopen mode must be ro or rw, not '{other}'"),
        };
        anyhow::ensure!(
            !host.is_empty() && guest.starts_with('/'),
            "--preopen needs a host path and an absolute guest path"
        );
        Ok(Self {
            host: host.to_owned(),
            guest: guest.to_owned(),
            access,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_are_denied_without_grants() {
        let grants = Grants::default();
        assert!(grants.notifications().is_err());
        assert!(grants.clipboard_read().is_err());
        assert!(grants.clipboard_write().is_err());
        assert!(grants.fetch_any().is_err());
        assert!(grants.fetch_host("example.org").is_err());
    }

    #[test]
    fn each_grant_opens_only_its_import() {
        let mut grants = Grants::default();
        grants.add("sys:clipboard:read").unwrap();
        assert!(grants.clipboard_read().is_ok());
        assert!(grants.clipboard_write().is_err());
        assert!(grants.notifications().is_err());
    }

    #[test]
    fn host_fetch_grants_match_exact_hosts() {
        let mut grants = Grants::default();
        grants.add("net:fetch:api.example.org").unwrap();
        assert!(grants.fetch_any().is_ok());
        assert!(grants.fetch_host("api.example.org").is_ok());
        assert!(grants.fetch_host("example.org").is_err());
        assert!(grants.fetch_host("evil.api.example.org").is_err());
        grants.add("net:fetch:*").unwrap();
        assert!(grants.fetch_host("example.org").is_ok());
    }

    #[test]
    fn filesystem_and_unknown_grants_are_refused() {
        let mut grants = Grants::default();
        assert!(grants.add("fs:rw:app-data").is_err());
        assert!(grants.add("hw:gpu:compute").is_err());
        assert!(grants.add("sys:everything").is_err());
    }

    #[test]
    fn preopens_carry_their_mode() {
        let preopen = Preopen::parse("/home/u/data::/data::ro").unwrap();
        assert_eq!(preopen.host, "/home/u/data");
        assert_eq!(preopen.guest, "/data");
        assert_eq!(preopen.access, Access::Read);
        assert_eq!(
            Preopen::parse("/a::/b::rw").unwrap().access,
            Access::ReadWrite
        );
        for bad in ["/a::/b", "/a", "/a::/b::wx", "::/b::ro", "/a::b::ro"] {
            assert!(Preopen::parse(bad).is_err(), "{bad} accepted");
        }
    }
}
