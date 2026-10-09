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
    /// Whether a request to `host` is covered by a fetch grant, and which
    /// addresses it may reach. `host` is the URL's host in the lowercase form
    /// URL parsing produces.
    pub fn fetch_host(&self, host: &str) -> Result<Reach, String> {
        if self.fetch.is_empty() {
            return Err("capability net:fetch is not granted".to_owned());
        }
        // An address named in a grant is reached as declared, wherever it
        // is; a name, or any host, reaches only public addresses, so a
        // granted name cannot be pointed at this machine or the local
        // network through DNS.
        if host.parse::<std::net::Ipv4Addr>().is_ok()
            && self
                .fetch
                .iter()
                .any(|scope| matches!(scope, FetchScope::Host(granted) if granted == host))
        {
            return Ok(Reach::Declared);
        }
        let covered = self.fetch.iter().any(|scope| match scope {
            FetchScope::AnyHost => true,
            FetchScope::Host(granted) => granted == host,
        });
        if covered {
            Ok(Reach::Public)
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
        // Guest paths are fixed names without `::`; splitting from the right
        // keeps a host path containing `::` whole.
        let (host, guest) = rest
            .rsplit_once("::")
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

/// The addresses a granted request may connect to.
#[cfg_attr(not(feature = "net-fetch"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Only public addresses.
    Public,
    /// The address the grant names, public or not.
    Declared,
}

/// Whether `ip` is a public unicast address: not loopback, unspecified,
/// private, shared (CGNAT), link-local, multicast, broadcast, reserved,
/// documentation or benchmarking space, nor an IPv6 form of one.
#[cfg_attr(not(feature = "net-fetch"), allow(dead_code))]
pub fn is_public(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (18..20).contains(&b)))
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            let embedded = |high: u16, low: u16| {
                IpAddr::V4(std::net::Ipv4Addr::new(
                    (high >> 8) as u8,
                    high as u8,
                    (low >> 8) as u8,
                    low as u8,
                ))
            };
            // Forms that carry an IPv4 address are judged by that address:
            // mapped and compatible addresses, 6to4 and NAT64.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            if seg[..6] == [0; 6] && !(seg[6] == 0 && seg[7] <= 1) {
                return is_public(embedded(seg[6], seg[7]));
            }
            if seg[0] == 0x2002 {
                return is_public(embedded(seg[1], seg[2]));
            }
            if seg[..6] == [0x0064, 0xff9b, 0, 0, 0, 0] {
                return is_public(embedded(seg[6], seg[7]));
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                // Unique local, link-local and the deprecated site-local.
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                || (seg[0] & 0xffc0) == 0xfec0
                // NAT64 for local use, discard-only.
                || (seg[0] == 0x0064 && seg[1] == 0xff9b)
                || (seg[0] == 0x0100 && seg[1..4] == [0, 0, 0])
                // Teredo, benchmarking, ORCHID and documentation.
                || (seg[0] == 0x2001 && seg[1] == 0)
                || (seg[0] == 0x2001 && seg[1] == 0x0002 && seg[2] == 0)
                || (seg[0] == 0x2001 && (seg[1] & 0xfff0) == 0x0010)
                || (seg[0] == 0x2001 && seg[1] == 0x0db8)
                || (seg[0] & 0xfff0) == 0x3ff0)
        }
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
    fn named_and_wildcard_grants_reach_only_public_addresses() {
        let mut grants = Grants::default();
        grants.add("net:fetch:api.example.org").unwrap();
        grants.add("net:fetch:127.0.0.1").unwrap();
        assert_eq!(grants.fetch_host("api.example.org"), Ok(Reach::Public));
        // An address named in a grant is reached as declared.
        assert_eq!(grants.fetch_host("127.0.0.1"), Ok(Reach::Declared));
        let mut any = Grants::default();
        any.add("net:fetch:*").unwrap();
        assert_eq!(any.fetch_host("10.0.0.1"), Ok(Reach::Public));
    }

    #[test]
    fn only_public_unicast_addresses_are_public() {
        // NAT64 and 6to4 forms of a public address are public, so DNS64
        // networks keep working.
        for public in [
            "93.184.216.34",
            "1.1.1.1",
            "2606:4700::1111",
            "64:ff9b::5db8:d822",
            "2002:5db8:d822::1",
        ] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        for local in [
            "127.0.0.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.1",
            "198.18.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "2001:db8::1",
            "fec0::1",
            "::7f00:1",
            "2002:7f00:1::",
            "2002:a00:1::",
            "2001::1",
            "2001:2::1",
            "2001:10::1",
            "3fff::1",
            "100::1",
        ] {
            assert!(!is_public(local.parse().unwrap()), "{local}");
        }
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
        assert_eq!(Preopen::parse("/a::/b::/data::ro").unwrap().host, "/a::/b");
        for bad in ["/a::/b", "/a", "/a::/b::wx", "::/b::ro", "/a::b::ro"] {
            assert!(Preopen::parse(bad).is_err(), "{bad} accepted");
        }
    }
}
