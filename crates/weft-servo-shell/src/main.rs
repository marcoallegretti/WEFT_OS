use std::path::PathBuf;

use anyhow::Context;

#[cfg(feature = "servo-embed")]
mod embedder;
mod protocols;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    run()
}

fn run() -> anyhow::Result<()> {
    let wayland_display = std::env::var("WAYLAND_DISPLAY")
        .context("WAYLAND_DISPLAY not set; weft-compositor must be running")?;

    tracing::info!(socket = %wayland_display, "connecting to Wayland compositor");

    let html_path = system_ui_html_path()?;
    tracing::info!(path = %html_path.display(), "system UI entry point located");

    let ws_port = appd_ws_port();
    tracing::info!(ws_port, "appd WebSocket port");

    embed_servo(&wayland_display, &html_path, ws_port)
}

fn system_ui_html_path() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("WEFT_SYSTEM_UI_HTML") {
        return Ok(PathBuf::from(p));
    }

    let packaged = PathBuf::from("/packages/system/servo-shell/active/share/weft/system-ui.html");
    if packaged.exists() {
        return Ok(packaged);
    }

    anyhow::bail!(
        "system-ui.html not found; set WEFT_SYSTEM_UI_HTML or install the servo-shell package"
    )
}

fn appd_ws_port() -> u16 {
    if let Ok(explicit) = std::env::var("WEFT_APPD_WS_PORT")
        && let Ok(n) = explicit.trim().parse::<u16>()
    {
        return n;
    }
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let port_file = std::path::Path::new(&runtime_dir)
            .join("weft")
            .join("appd.wsport");
        if let Ok(contents) = std::fs::read_to_string(&port_file)
            && let Ok(n) = contents.trim().parse::<u16>()
        {
            return n;
        }
    }
    7410
}

fn embed_servo(
    _wayland_display: &str,
    html_path: &std::path::Path,
    ws_port: u16,
) -> anyhow::Result<()> {
    #[cfg(feature = "servo-embed")]
    return embedder::run(html_path, ws_port);

    #[cfg(not(feature = "servo-embed"))]
    {
        tracing::warn!(
            path = %html_path.display(),
            ws_port,
            "servo-embed feature not enabled; build with --features servo-embed to activate"
        );
        anyhow::bail!("servo-embed feature required")
    }
}
