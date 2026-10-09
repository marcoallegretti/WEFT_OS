//! Capabilities an application declares in `wapp.toml`.
//!
//! This is the one vocabulary shared by package validation (weft-pack), the
//! session supervisor that derives effective grants (weft-appd) and the
//! runtime that enforces them (weft-runtime).

use std::fmt;
use std::str::FromStr;

/// Access mode of a filesystem capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    Read,
    ReadWrite,
}

/// Destinations covered by a fetch capability.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FetchScope {
    /// `net:fetch:*`: any host.
    AnyHost,
    /// `net:fetch:<host>`: exactly this host, in lowercase.
    Host(String),
}

/// One declared capability.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Capability {
    /// `fs:read:app-data` / `fs:rw:app-data`: the app's private data directory.
    AppData(Access),
    /// `fs:read:xdg-documents` / `fs:rw:xdg-documents`: the user's documents
    /// directory.
    Documents(Access),
    /// `net:fetch:*` / `net:fetch:<host>`: the `weft:app/fetch` import.
    Fetch(FetchScope),
    /// `sys:notifications`: the `weft:app/notifications` import.
    Notifications,
    /// `sys:clipboard:read`: `weft:app/clipboard#read`.
    ClipboardRead,
    /// `sys:clipboard:write`: `weft:app/clipboard#write`.
    ClipboardWrite,
    /// `hw:gpu:compute`.
    GpuCompute,
    /// `hw:gpu:render`.
    GpuRender,
}

/// A capability string outside the vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCapability(pub String);

impl fmt::Display for UnknownCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown capability '{}'", self.0)
    }
}

impl std::error::Error for UnknownCapability {}

impl FromStr for Capability {
    type Err = UnknownCapability;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let capability = match s {
            "fs:read:app-data" => Self::AppData(Access::Read),
            "fs:rw:app-data" => Self::AppData(Access::ReadWrite),
            "fs:read:xdg-documents" => Self::Documents(Access::Read),
            "fs:rw:xdg-documents" => Self::Documents(Access::ReadWrite),
            "net:fetch:*" => Self::Fetch(FetchScope::AnyHost),
            "sys:notifications" => Self::Notifications,
            "sys:clipboard:read" => Self::ClipboardRead,
            "sys:clipboard:write" => Self::ClipboardWrite,
            "hw:gpu:compute" => Self::GpuCompute,
            "hw:gpu:render" => Self::GpuRender,
            _ => match s.strip_prefix("net:fetch:") {
                Some(host) if is_host_name(host) => Self::Fetch(FetchScope::Host(host.to_owned())),
                _ => return Err(UnknownCapability(s.to_owned())),
            },
        };
        Ok(capability)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = |access: &Access| match access {
            Access::Read => "read",
            Access::ReadWrite => "rw",
        };
        match self {
            Self::AppData(access) => write!(f, "fs:{}:app-data", mode(access)),
            Self::Documents(access) => write!(f, "fs:{}:xdg-documents", mode(access)),
            Self::Fetch(FetchScope::AnyHost) => f.write_str("net:fetch:*"),
            Self::Fetch(FetchScope::Host(host)) => write!(f, "net:fetch:{host}"),
            Self::Notifications => f.write_str("sys:notifications"),
            Self::ClipboardRead => f.write_str("sys:clipboard:read"),
            Self::ClipboardWrite => f.write_str("sys:clipboard:write"),
            Self::GpuCompute => f.write_str("hw:gpu:compute"),
            Self::GpuRender => f.write_str("hw:gpu:render"),
        }
    }
}

/// A host in the canonical form URL parsing produces, so that a grant can
/// be compared with a request's host as a string: a lowercase DNS name of
/// 1–63 character labels of letters, digits and inner hyphens (at most 253
/// characters) whose last label is not numeric, or an IPv4 address in
/// dotted-decimal form without leading zeros. IPv6 literals are not
/// supported.
fn is_host_name(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    if labels
        .last()
        .is_some_and(|last| !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()))
    {
        return is_ipv4(&labels);
    }
    host.len() <= 253
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn is_ipv4(labels: &[&str]) -> bool {
    labels.len() == 4
        && labels.iter().all(|octet| {
            !octet.is_empty()
                && octet.len() <= 3
                && (octet.len() == 1 || !octet.starts_with('0'))
                && octet.bytes().all(|b| b.is_ascii_digit())
                && octet.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_round_trips() {
        for text in [
            "fs:read:app-data",
            "fs:rw:app-data",
            "fs:read:xdg-documents",
            "fs:rw:xdg-documents",
            "net:fetch:*",
            "net:fetch:api.example.org",
            "net:fetch:127.0.0.1",
            "net:fetch:a1.example",
            "sys:notifications",
            "sys:clipboard:read",
            "sys:clipboard:write",
            "hw:gpu:compute",
            "hw:gpu:render",
        ] {
            let capability: Capability = text.parse().unwrap();
            assert_eq!(capability.to_string(), text);
        }
    }

    #[test]
    fn malformed_capabilities_are_unknown() {
        for text in [
            "",
            "fs:write:app-data",
            "net:fetch",
            "net:fetch:",
            "net:fetch:Example.org",
            "net:fetch:example.org:443",
            "net:fetch:https://example.org",
            "net:fetch:*.example.org",
            "net:fetch:-bad.example",
            "net:fetch:a..b",
            "net:fetch:0177.0.0.1",
            "net:fetch:999.1.1.1",
            "net:fetch:1.2.3",
            "net:fetch:example.123",
            "net:fetch:[::1]",
            "sys:clipboard",
        ] {
            assert!(text.parse::<Capability>().is_err(), "{text:?} accepted");
        }
    }
}
