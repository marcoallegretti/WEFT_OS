use std::path::Path;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("mount") => {
            let img = args.get(2).context(
                "usage: weft-mount-helper mount <img> <hash_dev> <root_hash> <mountpoint>",
            )?;
            let hash_dev = args.get(3).context("missing <hash_dev>")?;
            let root_hash = args.get(4).context("missing <root_hash>")?;
            let mountpoint = args.get(5).context("missing <mountpoint>")?;
            cmd_mount(
                Path::new(img),
                Path::new(hash_dev),
                root_hash,
                Path::new(mountpoint),
            )?;
        }
        Some("umount") => {
            let mountpoint = args
                .get(2)
                .context("usage: weft-mount-helper umount <mountpoint>")?;
            cmd_umount(Path::new(mountpoint))?;
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  weft-mount-helper mount <img> <hash_dev> <root_hash> <mountpoint>");
            eprintln!("  weft-mount-helper umount <mountpoint>");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn effective_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("Uid:"))
        .and_then(|l| l.split_whitespace().nth(2))
        .and_then(|s| s.parse().ok())
}

fn require_root() -> anyhow::Result<()> {
    match effective_uid() {
        Some(0) => Ok(()),
        Some(uid) => anyhow::bail!("weft-mount-helper must run as root (euid={uid})"),
        None => {
            anyhow::bail!("weft-mount-helper must run as root (could not read /proc/self/status)")
        }
    }
}

/// The device-mapper name for an image mounted at `mountpoint`: `weft-`
/// followed by the mountpoint's last component. weft-appd names each
/// mountpoint with a random token, so concurrent sessions get distinct
/// devices. Device-mapper names are limited to 127 bytes.
fn device_name(mountpoint: &Path) -> String {
    let leaf = mountpoint
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let sanitized: String = leaf
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(120)
        .collect();
    format!("weft-{sanitized}")
}

fn cmd_mount(
    img: &Path,
    hash_dev: &Path,
    root_hash: &str,
    mountpoint: &Path,
) -> anyhow::Result<()> {
    require_root()?;
    let dev_name = device_name(mountpoint);

    let status = std::process::Command::new("veritysetup")
        .args([
            "open",
            &img.to_string_lossy(),
            &dev_name,
            &hash_dev.to_string_lossy(),
            root_hash,
        ])
        .status()
        .context("spawn veritysetup; ensure cryptsetup-bin is installed")?;
    if !status.success() {
        anyhow::bail!("veritysetup open failed with status {status}");
    }

    let mapper_dev = format!("/dev/mapper/{dev_name}");
    let status = std::process::Command::new("mount")
        .args([
            "-t",
            "erofs",
            "-o",
            "ro",
            &mapper_dev,
            &mountpoint.to_string_lossy(),
        ])
        .status()
        .context("spawn mount")?;
    if !status.success() {
        let _ = std::process::Command::new("veritysetup")
            .args(["close", &dev_name])
            .status();
        anyhow::bail!("mount failed with status {status}");
    }

    eprintln!("mounted: {} -> {}", img.display(), mountpoint.display());
    Ok(())
}

fn cmd_umount(mountpoint: &Path) -> anyhow::Result<()> {
    require_root()?;
    let dev_name = device_name(mountpoint);

    let status = std::process::Command::new("umount")
        .arg(mountpoint)
        .status()
        .context("spawn umount")?;
    if !status.success() {
        anyhow::bail!("umount failed with status {status}");
    }

    let status = std::process::Command::new("veritysetup")
        .args(["close", &dev_name])
        .status()
        .context("spawn veritysetup close; ensure cryptsetup-bin is installed")?;
    if !status.success() {
        anyhow::bail!("veritysetup close failed with status {status}");
    }

    eprintln!("unmounted: {}", mountpoint.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_follow_the_mountpoint_name() {
        let name = device_name(Path::new("/run/user/1000/weft/mnt/0123abcd"));
        assert_eq!(name, "weft-0123abcd");
        assert_ne!(
            device_name(Path::new("/run/user/1000/weft/mnt/aaaa")),
            device_name(Path::new("/run/user/1000/weft/mnt/bbbb"))
        );
    }

    #[test]
    fn device_names_are_sanitized_and_bounded() {
        let long = format!("/mnt/{}", "x.y".repeat(100));
        let name = device_name(Path::new(&long));
        assert!(name.len() <= 127);
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}
