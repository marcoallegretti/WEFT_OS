#[cfg(feature = "servo-embed")]
mod protocols;
#[cfg(feature = "servo-embed")]
mod shell_client;

#[cfg(feature = "servo-embed")]
mod embedder;
#[cfg(feature = "servo-embed")]
mod keyutils;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    // Standard output carries the readiness line to weft-appd, which keeps
    // standard error as the process's log.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    const USAGE: &str = "usage: weft-app-shell <app_id> <session_id> --ui <path>";
    let mut args = std::env::args().skip(1);
    let app_id = args.next().context(USAGE)?;
    let session_id: u64 = args
        .next()
        .context(USAGE)?
        .parse()
        .context("session_id must be a number")?;
    let ui = match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--ui"), Some(path), None) => std::path::PathBuf::from(path),
        _ => anyhow::bail!(USAGE),
    };

    let ws_port = appd_ws_port();

    embed_app(&app_id, session_id, ws_port, &ui)
}

fn appd_ws_port() -> u16 {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let port_file = std::path::PathBuf::from(runtime_dir).join("weft/appd.wsport");
        if let Ok(s) = std::fs::read_to_string(port_file)
            && let Ok(n) = s.trim().parse()
        {
            return n;
        }
    }
    if let Ok(s) = std::env::var("WEFT_APPD_WS_PORT")
        && let Ok(n) = s.parse()
    {
        return n;
    }
    7410
}

fn embed_app(
    app_id: &str,
    session_id: u64,
    ws_port: u16,
    ui: &std::path::Path,
) -> anyhow::Result<()> {
    #[cfg(feature = "servo-embed")]
    return embedder::run(app_id, session_id, ws_port, ui);

    #[cfg(not(feature = "servo-embed"))]
    {
        let _ = (app_id, session_id, ws_port, ui);
        anyhow::bail!("weft-app-shell was built without the servo-embed feature and cannot render")
    }
}
