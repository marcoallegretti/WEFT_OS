use std::path::{Path, PathBuf};

fn mount_helper_bin() -> Option<String> {
    if let Ok(v) = std::env::var("WEFT_MOUNT_HELPER") {
        return Some(v);
    }
    for candidate in [
        "/usr/lib/weft/weft-mount-helper",
        "/usr/local/lib/weft/weft-mount-helper",
    ] {
        if Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    None
}

pub struct MountOrchestrator {
    mountpoint: Option<PathBuf>,
}

impl MountOrchestrator {
    pub fn mount_if_needed(app_id: &str, session_id: u64) -> (Self, Option<PathBuf>) {
        let Some(helper) = mount_helper_bin() else {
            return (Self { mountpoint: None }, None);
        };

        let (img, hash_dev, root_hash) = match find_image(app_id) {
            Some(t) => t,
            None => return (Self { mountpoint: None }, None),
        };

        let base = std::env::temp_dir().join(format!("weft-mnt-{session_id}"));
        let mountpoint = base.join(app_id);

        if let Err(e) = std::fs::create_dir_all(&mountpoint) {
            tracing::warn!(session_id, %app_id, error=%e, "cannot create mount dir; skipping image mount");
            return (Self { mountpoint: None }, None);
        }

        let status = std::process::Command::new(&helper)
            .args([
                "mount",
                &img.to_string_lossy(),
                &hash_dev.to_string_lossy(),
                &root_hash,
                &mountpoint.to_string_lossy(),
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                tracing::info!(session_id, %app_id, path=%mountpoint.display(), "EROFS image mounted");
                (
                    Self {
                        mountpoint: Some(mountpoint),
                    },
                    Some(base),
                )
            }
            Ok(s) => {
                tracing::warn!(session_id, %app_id, status=%s, "mount-helper failed; using directory install");
                let _ = std::fs::remove_dir_all(&base);
                (Self { mountpoint: None }, None)
            }
            Err(e) => {
                tracing::warn!(session_id, %app_id, error=%e, "spawn mount-helper failed; using directory install");
                let _ = std::fs::remove_dir_all(&base);
                (Self { mountpoint: None }, None)
            }
        }
    }

    pub fn umount(&self) {
        let Some(ref mp) = self.mountpoint else {
            return;
        };
        let Some(helper) = mount_helper_bin() else {
            return;
        };
        let _ = std::process::Command::new(&helper)
            .args(["umount", &mp.to_string_lossy()])
            .status();
        if let Some(parent) = mp.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

fn find_image(app_id: &str) -> Option<(PathBuf, PathBuf, String)> {
    for root in crate::app_store_roots() {
        let img = root.join(format!("{app_id}.app.img"));
        let hash_dev = root.join(format!("{app_id}.hash"));
        let roothash_file = root.join(format!("{app_id}.roothash"));
        if img.exists() && hash_dev.exists() && roothash_file.exists() {
            let Ok(root_hash) = std::fs::read_to_string(&roothash_file) else {
                continue;
            };
            return Some((img, hash_dev, root_hash.trim().to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_image_returns_none_when_absent() {
        unsafe { std::env::set_var("WEFT_APP_STORE", "/tmp/nonexistent_weft_store_xyz") };
        let result = find_image("com.example.missing");
        unsafe { std::env::remove_var("WEFT_APP_STORE") };
        assert!(result.is_none());
    }
}
