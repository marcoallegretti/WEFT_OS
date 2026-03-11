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
        Some("uninstall") => {
            let app_id = args.get(2).context("usage: weft-pack uninstall <app_id>")?;
            uninstall_package(app_id)?;
        }
        Some("list") => {
            list_installed();
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  weft-pack check     <dir>     validate a package directory");
            eprintln!("  weft-pack info      <dir>     print package metadata");
            eprintln!("  weft-pack install   <dir>     install package to app store");
            eprintln!("  weft-pack uninstall <app_id>  remove installed package");
            eprintln!("  weft-pack list                list installed packages");
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
        } else if !is_wasm_module(&wasm_path) {
            errors.push(format!(
                "runtime.module '{}' is not a valid Wasm module (bad magic bytes)",
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

fn is_wasm_module(path: &Path) -> bool {
    const MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
    let mut buf = [0u8; 4];
    match std::fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read_exact(&mut buf)
                .map(|_| buf == MAGIC)
                .unwrap_or(false)
        }
        Err(_) => false,
    }
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
    let root = resolve_install_root()?;
    install_package_to(dir, &root)
}

fn install_package_to(dir: &Path, store_root: &Path) -> anyhow::Result<()> {
    check_package(dir)?;
    let manifest = load_manifest(dir)?;
    let app_id = &manifest.package.id;
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

fn uninstall_package(app_id: &str) -> anyhow::Result<()> {
    let root = resolve_install_root()?;
    uninstall_package_from(app_id, &root)
}

fn uninstall_package_from(app_id: &str, store_root: &Path) -> anyhow::Result<()> {
    if !is_valid_app_id(app_id) {
        anyhow::bail!("'{}' is not a valid app ID", app_id);
    }
    let target = store_root.join(app_id);
    if !target.exists() {
        anyhow::bail!(
            "package '{}' is not installed at {}",
            app_id,
            target.display()
        );
    }
    std::fs::remove_dir_all(&target).with_context(|| format!("remove {}", target.display()))?;
    println!("uninstalled {}", app_id);
    Ok(())
}

fn list_installed_roots() -> Vec<PathBuf> {
    if let Ok(explicit) = std::env::var("WEFT_APP_STORE") {
        return vec![PathBuf::from(explicit)];
    }
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("weft")
                .join("apps"),
        );
    }
    roots.push(PathBuf::from("/usr/share/weft/apps"));
    roots
}

fn list_installed() {
    let mut seen = std::collections::HashSet::new();
    let mut count = 0usize;
    for root in list_installed_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut pkgs: Vec<(String, String, String)> = Vec::new();
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("wapp.toml");
            let Ok(contents) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Ok(m) = toml::from_str::<Manifest>(&contents) else {
                continue;
            };
            if seen.insert(m.package.id.clone()) {
                pkgs.push((m.package.id, m.package.name, m.package.version));
            }
        }
        pkgs.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, name, version) in pkgs {
            println!("{id}  {name}  {version}");
            count += 1;
        }
    }
    if count == 0 {
        println!("no packages installed");
    }
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
    fn install_package_copies_to_store() {
        use std::fs;
        let id = format!("weft.pack.install{}", std::process::id());
        let src = std::env::temp_dir().join(format!("weft_pack_install_src_{}", id));
        let store = std::env::temp_dir().join(format!("weft_pack_install_store_{}", id));
        let ui_dir = src.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(src.join("app.wasm"), b"\0asm").unwrap();
        fs::write(ui_dir.join("index.html"), b"<!DOCTYPE html>").unwrap();
        let app_id = format!("com.example.t{}", std::process::id());
        fs::write(
            src.join("wapp.toml"),
            format!(
                "[package]\nid = \"{app_id}\"\nname = \"Test\"\nversion = \"1.0.0\"\n\n\
                 [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n"
            ),
        )
        .unwrap();
        let result = install_package_to(&src, &store);
        assert!(result.is_ok(), "{result:?}");
        assert!(store.join(&app_id).join("app.wasm").exists());
        assert!(store.join(&app_id).join("wapp.toml").exists());
        assert!(store.join(&app_id).join("ui").join("index.html").exists());
        assert!(install_package_to(&src, &store).is_err());
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn uninstall_package_removes_directory() {
        use std::fs;
        let id = format!("weft.pack.uninstall{}", std::process::id());
        let src = std::env::temp_dir().join(format!("weft_pack_uninstall_src_{}", id));
        let store = std::env::temp_dir().join(format!("weft_pack_uninstall_store_{}", id));
        let ui_dir = src.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(src.join("app.wasm"), b"\0asm").unwrap();
        fs::write(ui_dir.join("index.html"), b"").unwrap();
        let app_id = format!("com.example.u{}", std::process::id());
        fs::write(
            src.join("wapp.toml"),
            format!(
                "[package]\nid = \"{app_id}\"\nname = \"U\"\nversion = \"1.0.0\"\n\n\
                 [runtime]\nmodule = \"app.wasm\"\n\n[ui]\nentry = \"ui/index.html\"\n"
            ),
        )
        .unwrap();
        install_package_to(&src, &store).unwrap();
        assert!(store.join(&app_id).exists());
        let result = uninstall_package_from(&app_id, &store);
        assert!(result.is_ok(), "{result:?}");
        assert!(!store.join(&app_id).exists());
        assert!(uninstall_package_from(&app_id, &store).is_err());
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn check_package_bad_wasm_magic() {
        use std::fs;
        let tmp = std::env::temp_dir().join("weft_pack_test_bad_wasm");
        let ui_dir = tmp.join("ui");
        let _ = fs::create_dir_all(&ui_dir);
        fs::write(tmp.join("app.wasm"), b"NOT_WASM").unwrap();
        fs::write(ui_dir.join("index.html"), b"").unwrap();
        fs::write(
            tmp.join("wapp.toml"),
            r#"
[package]
id = "com.example.badwasm"
name = "Bad Wasm"
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
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("bad magic bytes"), "got: {msg}");
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

    #[test]
    fn list_installed_roots_uses_weft_app_store_when_set() {
        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::set_var("WEFT_APP_STORE", "/custom/store") };
        let roots = list_installed_roots();
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
        }
        assert_eq!(roots, vec![PathBuf::from("/custom/store")]);
    }

    #[test]
    fn list_installed_roots_includes_system_path() {
        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::remove_var("WEFT_APP_STORE") };
        let roots = list_installed_roots();
        unsafe {
            if let Some(v) = prior {
                std::env::set_var("WEFT_APP_STORE", v);
            }
        }
        assert!(
            roots
                .iter()
                .any(|p| p == &PathBuf::from("/usr/share/weft/apps"))
        );
    }
}
