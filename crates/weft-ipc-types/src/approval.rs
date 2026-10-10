//! The capabilities the user approved for an installed app.
//!
//! A package declares capabilities; it gets only those the user approved.
//! `weft-pack` records the approval, `$XDG_DATA_HOME/weft/approvals/<id>`,
//! one capability per line, and weft-appd refuses to launch an app that
//! declares a capability the record does not hold. An app's own private
//! data is a minimal execution resource and needs no approval. Signature
//! verification and approval are separate decisions: a trusted signature
//! approves nothing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::capability::Capability;

/// Where the approval record of `app_id` lives.
pub fn approval_path(data_home: &Path, app_id: &str) -> PathBuf {
    data_home.join("weft/approvals").join(app_id)
}

/// Whether a declared capability needs the user's approval. Strings that
/// are not capabilities are left to the capability checks, which refuse
/// them.
pub fn needs_approval(capability: &str) -> bool {
    capability
        .parse::<Capability>()
        .is_ok_and(|c| !matches!(c, Capability::AppData(_)))
}

/// The declared capabilities that need approval and are not in `approved`.
pub fn unapproved<'a>(declared: &'a [String], approved: &BTreeSet<String>) -> Vec<&'a str> {
    declared
        .iter()
        .map(String::as_str)
        .filter(|c| needs_approval(c) && !approved.contains(*c))
        .collect()
}

/// The capabilities approved for an app; none when there is no record.
pub fn read_approved(record: &Path) -> std::io::Result<BTreeSet<String>> {
    match std::fs::read_to_string(record) {
        Ok(text) => Ok(text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e),
    }
}

/// Records exactly the capabilities of `declared` that need approval as the
/// app's approval, replacing an earlier one in one rename. With none to
/// approve, the record is removed.
pub fn write_approved(record: &Path, declared: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let approved: BTreeSet<&str> = declared
        .iter()
        .map(String::as_str)
        .filter(|c| needs_approval(c))
        .collect();
    if approved.is_empty() {
        return remove_approval(record);
    }
    let parent = record
        .parent()
        .expect("approval records live in a directory");
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        record
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("record"),
        std::process::id()
    ));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        for capability in &approved {
            writeln!(file, "{capability}")?;
        }
        file.sync_all()?;
        std::fs::rename(&temp, record)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written?;
    std::fs::File::open(parent).and_then(|dir| dir.sync_all())
}

/// Removes an app's approval, so a later installation asks again.
pub fn remove_approval(record: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(record) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(list: &[&str]) -> Vec<String> {
        list.iter().map(|c| (*c).to_owned()).collect()
    }

    #[test]
    fn private_app_data_needs_no_approval() {
        assert!(!needs_approval("fs:rw:app-data"));
        assert!(!needs_approval("fs:read:app-data"));
        assert!(needs_approval("net:fetch:example.org"));
        assert!(needs_approval("sys:notifications"));
        assert!(needs_approval("fs:read:xdg-documents"));
        // Not a capability: refused elsewhere, not reported as unapproved.
        assert!(!needs_approval("sys:everything"));
    }

    #[test]
    fn approvals_cover_exactly_what_was_approved() {
        let dir = std::env::temp_dir().join(format!("weft_approval_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let record = approval_path(&dir, "org.weft.test");
        let declared = caps(&["fs:rw:app-data", "net:fetch:example.org"]);
        assert_eq!(read_approved(&record).unwrap(), BTreeSet::new());
        assert_eq!(
            unapproved(&declared, &read_approved(&record).unwrap()),
            ["net:fetch:example.org"]
        );
        write_approved(&record, &declared).unwrap();
        assert!(unapproved(&declared, &read_approved(&record).unwrap()).is_empty());
        // An update that asks for more needs approval for the new capability.
        let updated = caps(&["net:fetch:example.org", "sys:clipboard:read"]);
        assert_eq!(
            unapproved(&updated, &read_approved(&record).unwrap()),
            ["sys:clipboard:read"]
        );
        // Nothing left to approve removes the record.
        write_approved(&record, &caps(&["fs:rw:app-data"])).unwrap();
        assert!(!record.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
