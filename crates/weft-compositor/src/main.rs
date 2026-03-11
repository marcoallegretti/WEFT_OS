use tracing_subscriber::EnvFilter;

mod backend;
mod input;
mod protocols;
mod state;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let use_winit = args.iter().any(|a| a == "--winit")
        || std::env::var("DISPLAY").is_ok()
        || std::env::var("WAYLAND_DISPLAY").is_ok();

    if use_winit {
        tracing::info!("starting compositor with winit backend");
        backend::winit::run()
    } else {
        tracing::info!("starting compositor with DRM/KMS backend");
        backend::drm::run()
    }
}
