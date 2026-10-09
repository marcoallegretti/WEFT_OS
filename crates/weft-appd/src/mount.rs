//! Verified package images, mounted read-only for the session that runs
//! them.

use std::path::{Path, PathBuf};

use weft_ipc_types::package::ImageFiles;

fn mount_helper_bin() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WEFT_MOUNT_HELPER") {
        return Some(PathBuf::from(path));
    }
    [
        "/usr/lib/weft/weft-mount-helper",
        "/usr/local/lib/weft/weft-mount-helper",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|candidate| candidate.exists())
}

/// A mounted image. Dropping it unmounts the image and removes the empty
/// mountpoint; a mountpoint that cannot be unmounted is left in place.
pub(crate) struct Mount {
    helper: PathBuf,
    mountpoint: PathBuf,
}

impl Mount {
    /// The package root inside the image.
    pub(crate) fn root(&self) -> &Path {
        &self.mountpoint
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let unmounted = std::process::Command::new(&self.helper)
            .arg("umount")
            .arg(&self.mountpoint)
            .status()
            .is_ok_and(|status| status.success());
        if !unmounted {
            tracing::error!(path = %self.mountpoint.display(), "cannot unmount package image");
            return;
        }
        if let Err(e) = std::fs::remove_dir(&self.mountpoint) {
            tracing::warn!(path = %self.mountpoint.display(), error = %e, "cannot remove mountpoint");
        }
    }
}

/// Mounts the verified image `files` of `app_id`. Every failure is returned:
/// the caller must not fall back to another copy of the package.
pub(crate) fn mount(app_id: &str, files: &ImageFiles) -> Result<Mount, String> {
    let helper = mount_helper_bin().ok_or("weft-mount-helper is not installed")?;
    for path in [&files.image, &files.hash_tree, &files.root_hash] {
        let is_file = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file());
        if !is_file {
            return Err(format!(
                "{} is missing or not a regular file",
                path.display()
            ));
        }
    }
    let root_hash = std::fs::read_to_string(&files.root_hash)
        .map_err(|e| format!("cannot read {}: {e}", files.root_hash.display()))?;
    let root_hash = root_hash.trim();
    if !is_root_hash(root_hash) {
        return Err(format!("{} holds no root hash", files.root_hash.display()));
    }

    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    let base = PathBuf::from(runtime_dir).join("weft/mnt");
    crate::private_dir(&base).map_err(|e| format!("cannot create {}: {e}", base.display()))?;
    let name = crate::runtime::random_token().map_err(|e| format!("no mountpoint name: {e}"))?;
    let mountpoint = base.join(name);
    std::fs::create_dir(&mountpoint)
        .map_err(|e| format!("cannot create {}: {e}", mountpoint.display()))?;

    let status = std::process::Command::new(&helper)
        .arg("mount")
        .arg(&files.image)
        .arg(&files.hash_tree)
        .arg(root_hash)
        .arg(&mountpoint)
        .status();
    match status {
        Ok(status) if status.success() => {
            tracing::info!(%app_id, path = %mountpoint.display(), "package image mounted");
            Ok(Mount { helper, mountpoint })
        }
        outcome => {
            let _ = std::fs::remove_dir(&mountpoint);
            Err(match outcome {
                Ok(status) => format!("weft-mount-helper failed: {status}"),
                Err(e) => format!("cannot run {}: {e}", helper.display()),
            })
        }
    }
}

/// A dm-verity root hash in hex, as `veritysetup format` prints it.
fn is_root_hash(text: &str) -> bool {
    (64..=128).contains(&text.len())
        && text.len().is_multiple_of(2)
        && text.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_hashes_are_hex_digests() {
        assert!(is_root_hash(&"a".repeat(64)));
        assert!(is_root_hash(&"0F".repeat(64)));
        for bad in [
            "",
            "abc",
            &"g".repeat(64),
            &"a".repeat(65),
            &"a".repeat(130),
        ] {
            assert!(!is_root_hash(bad), "{bad:?}");
        }
    }
}
