mod protocols;
mod shell_client;

#[cfg(feature = "servo-embed")]
mod embedder;
#[cfg(feature = "servo-embed")]
mod keyutils;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let app_id = args
        .next()
        .context("usage: weft-app-shell <app_id> <session_id>")?;
    let session_id: u64 = args
        .next()
        .context("usage: weft-app-shell <app_id> <session_id>")?
        .parse()
        .context("session_id must be a number")?;

    let ws_port = appd_ws_port();

    embed_app(&app_id, session_id, ws_port)
}

fn appd_ws_port() -> u16 {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let port_file = std::path::PathBuf::from(runtime_dir).join("weft/appd.wsport");
        if let Ok(s) = std::fs::read_to_string(port_file) {
            if let Ok(n) = s.trim().parse() {
                return n;
            }
        }
    }
    if let Ok(s) = std::env::var("WEFT_APPD_WS_PORT") {
        if let Ok(n) = s.parse() {
            return n;
        }
    }
    7410
}

fn embed_app(app_id: &str, session_id: u64, ws_port: u16) -> anyhow::Result<()> {
    #[cfg(feature = "servo-embed")]
    return embedder::run(app_id, session_id, ws_port);

    #[cfg(not(feature = "servo-embed"))]
    {
        let _ = (app_id, session_id, ws_port);
        println!("READY");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::thread::park();
        Ok(())
    }
}
