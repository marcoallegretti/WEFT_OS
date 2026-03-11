use std::path::PathBuf;

use anyhow::Context;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    run().await
}

async fn run() -> anyhow::Result<()> {
    let socket_path = appd_socket_path()?;
    tracing::info!(path = %socket_path.display(), "weft-appd IPC socket");

    // Wave 5 skeleton entry point.
    //
    // Components to implement (see docu_dev/WEFT-OS-APPD-DESIGN.md):
    //   AppRegistry        — in-memory map of running app sessions and state
    //   IpcServer          — Unix socket at socket_path, serves servo-shell requests
    //   CompositorClient   — Unix socket client to weft-compositor IPC server
    //   RuntimeSupervisor  — spawns and monitors Wasmtime child processes
    //   CapabilityBroker   — resolves manifest permissions to runtime handles
    //   ResourceController — configures cgroups via systemd transient units
    //
    // IPC transport: 4-byte LE length-prefixed MessagePack frames.
    // Socket path: /run/user/<uid>/weft/appd.sock (overridable via WEFT_APPD_SOCKET).
    //
    // sd_notify(READY=1) is sent after IpcServer is bound and CompositorClient
    // has established its connection, satisfying Type=notify in weft-appd.service.
    anyhow::bail!(
        "weft-appd event loop not yet implemented; \
         see docu_dev/WEFT-OS-APPD-DESIGN.md"
    )
}

fn appd_socket_path() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("WEFT_APPD_SOCKET") {
        return Ok(PathBuf::from(p));
    }

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR not set")?;

    Ok(PathBuf::from(runtime_dir).join("weft/appd.sock"))
}
