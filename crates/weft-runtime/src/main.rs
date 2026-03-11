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

    tracing::info!(session_id, %app_id, wasm = %wasm_path.display(), "executing module");
    run_module(&wasm_path)?;

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

#[cfg(not(feature = "wasmtime-runtime"))]
fn run_module(_wasm_path: &std::path::Path) -> anyhow::Result<()> {
    println!("READY");
    Ok(())
}

#[cfg(feature = "wasmtime-runtime")]
fn run_module(wasm_path: &std::path::Path) -> anyhow::Result<()> {
    use wasmtime::{
        Config, Engine, Store,
        component::{Component, Linker},
    };
    use wasmtime_wasi::{
        ResourceTable, WasiCtx, WasiCtxBuilder, WasiView, add_to_linker_sync,
        bindings::sync::Command,
    };

    struct State {
        ctx: WasiCtx,
        table: ResourceTable,
    }

    impl WasiView for State {
        fn ctx(&mut self) -> &mut WasiCtx {
            &mut self.ctx
        }
        fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).context("create engine")?;

    let component = Component::from_file(&engine, wasm_path)
        .with_context(|| format!("load component {}", wasm_path.display()))?;

    let mut linker: Linker<State> = Linker::new(&engine);
    add_to_linker_sync(&mut linker).context("add WASI to linker")?;

    let ctx = WasiCtxBuilder::new()
        .inherit_stdout()
        .inherit_stderr()
        .build();
    let mut store = Store::new(
        &engine,
        State {
            ctx,
            table: ResourceTable::new(),
        },
    );

    println!("READY");

    let command =
        Command::instantiate(&mut store, &component, &linker).context("instantiate component")?;

    command
        .wasi_cli_run()
        .call_run(&mut store)
        .context("call run")?
        .map_err(|()| anyhow::anyhow!("wasm component run exited with error"))
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

    #[test]
    fn resolve_package_finds_installed_package() {
        use std::fs;
        let store =
            std::env::temp_dir().join(format!("weft_runtime_resolve_{}", std::process::id()));
        let pkg_dir = store.join("com.example.resolve");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("wapp.toml"),
            "[package]\nid=\"com.example.resolve\"\n",
        )
        .unwrap();

        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::set_var("WEFT_APP_STORE", &store) };

        let result = resolve_package("com.example.resolve");

        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
        }
        let _ = fs::remove_dir_all(&store);

        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("com.example.resolve"));
    }

    #[test]
    fn resolve_package_errors_on_unknown_id() {
        let store =
            std::env::temp_dir().join(format!("weft_runtime_resolve_empty_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&store);

        let prior = std::env::var("WEFT_APP_STORE").ok();
        unsafe { std::env::set_var("WEFT_APP_STORE", &store) };

        let result = resolve_package("com.does.not.exist");

        unsafe {
            match prior {
                Some(v) => std::env::set_var("WEFT_APP_STORE", v),
                None => std::env::remove_var("WEFT_APP_STORE"),
            }
        }
        let _ = std::fs::remove_dir_all(&store);

        assert!(result.is_err());
    }
}
