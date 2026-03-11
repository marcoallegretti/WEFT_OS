use std::path::PathBuf;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        anyhow::bail!("usage: weft-runtime <app_id> <session_id>");
    }
    let app_id = &args[1];
    let session_id: u64 = args[2]
        .parse()
        .with_context(|| format!("invalid session_id: {}", args[2]))?;

    tracing::info!(session_id, %app_id, "weft-runtime starting");

    let pkg_dir = resolve_package(app_id)?;
    tracing::info!(path = %pkg_dir.display(), "package resolved");

    let wasm_path = pkg_dir.join("app.wasm");
    if !wasm_path.exists() {
        anyhow::bail!("app.wasm not found at {}", wasm_path.display());
    }

    // TODO: Load wasm_path into a Wasmtime Engine and run the module.
    // Until Wasmtime is integrated, print READY and exit cleanly so that
    // weft-appd can complete the session lifecycle in tests and development.
    tracing::info!(session_id, %app_id, wasm = %wasm_path.display(), "Wasmtime integration pending");

    println!("READY");

    tracing::info!(session_id, %app_id, "exiting");
    Ok(())
}

fn resolve_package(app_id: &str) -> anyhow::Result<PathBuf> {
    for store_root in package_store_roots() {
        let pkg_dir = store_root.join(app_id);
        let manifest = pkg_dir.join("wapp.toml");
        if manifest.exists() {
            return Ok(pkg_dir);
        }
    }
    anyhow::bail!("package '{}' not found in any package store", app_id)
}

fn package_store_roots() -> Vec<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_store_roots_includes_system_path() {
        let roots = package_store_roots();
        assert!(
            roots
                .iter()
                .any(|p| p == &PathBuf::from("/usr/share/weft/apps"))
        );
    }

    #[test]
    fn package_store_roots_uses_weft_app_store_when_set() {
        // SAFETY: test binary is single-threaded at this point.
        unsafe { std::env::set_var("WEFT_APP_STORE", "/custom/store") };
        let roots = package_store_roots();
        assert_eq!(roots, vec![PathBuf::from("/custom/store")]);
        unsafe { std::env::remove_var("WEFT_APP_STORE") };
    }
}
