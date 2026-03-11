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

fn device_name(mountpoint: &Path) -> String {
    let s = mountpoint.to_string_lossy();
    let sanitized: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let suffix: String = sanitized
        .trim_matches('-')
        .chars()
        .take(26)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string();
    format!("weft-{suffix}")
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
    fn device_name_sanitizes_path() {
        let mp = Path::new("/run/weft/mounts/com.example.myapp");
        let name = device_name(mp);
        assert!(name.starts_with("weft-"));
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert!(name.len() <= 31);
    }

    #[test]
    fn device_name_truncates_long_paths() {
        let mp = Path::new("/run/weft/mounts/com.example.averylongappidthatexceedsthemaximum");
        let name = device_name(mp);
        assert!(name.len() <= 31);
    }
}
