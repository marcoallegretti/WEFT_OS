use std::path::PathBuf;

use anyhow::Context;

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

    embed_servo(&wayland_display, &html_path)
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

fn embed_servo(_wayland_display: &str, _html_path: &std::path::Path) -> anyhow::Result<()> {
    // Wave 4 skeleton entry point.
    //
    // Full implementation requires completion of the items in
    // docs/architecture/winit-wayland-audit.md before production readiness,
    // and the following integration work:
    //
    //   1. Add servo git dependency (not on crates.io; requires building Servo)
    //   2. Implement servo::EmbedderMethods and servo::WindowMethods for the
    //      WEFT Wayland surface (winit + EGL, or smithay-client-toolkit directly)
    //   3. Call servo::Servo::new() with the window and embedder
    //   4. Load the system UI via servo::ServoUrl::parse(html_path)
    //   5. Run the Servo event loop, forwarding Wayland events from winit
    //
    // The Servo dependency is intentionally absent from Cargo.toml at this stage.
    // It requires a git dependency on github.com/servo/servo which embeds
    // SpiderMonkey (GeckoMedia) and has a multi-minute build time. It is added
    // when the embedder contract is ready.
    anyhow::bail!(
        "Servo embedding not yet implemented; \
         see docs/architecture/winit-wayland-audit.md for gap assessment"
    )
}
