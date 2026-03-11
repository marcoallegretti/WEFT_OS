use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Manifest {
    package: PackageMeta,
    runtime: RuntimeMeta,
    ui: UiMeta,
}

#[derive(Debug, Deserialize)]
struct PackageMeta {
    id: String,
    name: String,
    version: String,
    description: Option<String>,
    author: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuntimeMeta {
    module: String,
}

#[derive(Debug, Deserialize)]
struct UiMeta {
    entry: String,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("check") => {
            let dir = args.get(2).context("usage: weft-pack check <dir>")?;
            let result = check_package(Path::new(dir))?;
            println!("{result}");
        }
        Some("info") => {
            let dir = args.get(2).context("usage: weft-pack info <dir>")?;
            let manifest = load_manifest(Path::new(dir))?;
            print_info(&manifest);
        }
        Some("install") => {
            let dir = args.get(2).context("usage: weft-pack install <dir>")?;
            install_package(Path::new(dir))?;
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  weft-pack check   <dir>   validate a package directory");
            eprintln!("  weft-pack info    <dir>   print package metadata");
            eprintln!("  weft-pack install <dir>   install package to app store");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn check_package(dir: &Path) -> anyhow::Result<String> {
    let mut errors: Vec<String> = Vec::new();

    let manifest = match load_manifest(dir) {
        Ok(m) => Some(m),
        Err(e) => {
            errors.push(format!("wapp.toml: {e}"));
            None
        }
    };

    if let Some(ref m) = manifest {
        if !is_valid_app_id(&m.package.id) {
            errors.push(format!(
                "package.id '{}' does not match required pattern",
                m.package.id
            ));
        }
        if m.package.name.is_empty() {
            errors.push("package.name is empty".into());
        }
        if m.package.name.len() > 64 {
            errors.push(format!(
                "package.name exceeds 64 characters ({})",
                m.package.name.len()
            ));
        }

        let wasm_path = dir.join(&m.runtime.module);
        if !wasm_path.exists() {
            errors.push(format!(
                "runtime.module '{}' not found",
                wasm_path.display()
            ));
        }

        let ui_path = dir.join(&m.ui.entry);
        if !ui_path.exists() {
            errors.push(format!("ui.entry '{}' not found", ui_path.display()));
        }
    }

    if errors.is_empty() {
        Ok("OK".into())
    } else {
        Err(anyhow::anyhow!("{}", errors.join("\n")))
    }
}

fn load_manifest(dir: &Path) -> anyhow::Result<Manifest> {
    let manifest_path = dir.join("wapp.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", manifest_path.display()))
}

fn print_info(m: &Manifest) {
    println!("id:      {}", m.package.id);
    println!("name:    {}", m.package.name);
    println!("version: {}", m.package.version);
    if let Some(ref d) = m.package.description {
        println!("desc:    {d}");
    }
    if let Some(ref a) = m.package.author {
        println!("author:  {a}");
    }
    println!("module:  {}", m.runtime.module);
    println!("ui:      {}", m.ui.entry);
}

fn is_valid_app_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() < 3 {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty()
            && p.chars()
                .next()
                .map(|c| c.is_ascii_lowercase())
                .unwrap_or(false)
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

fn resolve_install_root() -> anyhow::Result<PathBuf> {
    if let Ok(explicit) = std::env::var("WEFT_APP_STORE") {
        return Ok(PathBuf::from(explicit));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("weft")
            .join("apps"));
    }
    anyhow::bail!("cannot determine install root: HOME and WEFT_APP_STORE are both unset")
}

fn install_package(dir: &Path) -> anyhow::Result<()> {
    check_package(dir)?;
    let manifest = load_manifest(dir)?;
    let app_id = &manifest.package.id;

    let store_root = resolve_install_root()?;
    let dest = store_root.join(app_id);

    if dest.exists() {
        anyhow::bail!(
            "package '{}' is already installed at {}; remove it first",
            app_id,
            dest.display()
        );
    }

    copy_dir(dir, &dest)
        .with_context(|| format!("copy {} -> {}", dir.display(), dest.display()))?;

    println!("installed {} -> {}", app_id, dest.display());
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .with_context(|| format!("copy {}", src_path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_valid() {
        assert!(is_valid_app_id("com.example.notes"));
        assert!(is_valid_app_id("org.weft.calculator"));
        assert!(is_valid_app_id("io.github.username.app"));
    }

    #[test]
    fn app_id_invalid() {
        assert!(!is_valid_app_id("com.example"));
        assert!(!is_valid_app_id("Com.example.notes"));
        assert!(!is_valid_app_id("com.example.notes-app"));
        assert!(!is_valid_app_id("com..example.notes"));
        assert!(!is_valid_app_id(""));
        assert!(!is_valid_app_id("com.Example.notes"));
    }

    #[test]
    fn check_package_missing_manifest() {
        let tmp = std::env::temp_dir().join("weft_pack_test_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let result = check_package(&tmp);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_package_valid() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_valid");
        let ui_dir = tmp.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(tmp.join("app.wasm"), b"\0asm\x01\0\0\0").unwrap();
        fs::write(ui_dir.join("index.html"), b"<!DOCTYPE html>").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "com.example.test"
name = "Test App"
version = "1.0.0"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
"#,
        )
        .unwrap();

        let result = check_package(&tmp);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(result.unwrap(), "OK");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_package_invalid_app_id() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_invalid_id");
        let ui_dir = tmp.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(tmp.join("app.wasm"), b"\0asm").unwrap();
        fs::write(ui_dir.join("index.html"), b"").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "bad-id"
name = "Bad"
version = "0.1.0"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
"#,
        )
        .unwrap();

        let result = check_package(&tmp);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&tmp);
    }
}
