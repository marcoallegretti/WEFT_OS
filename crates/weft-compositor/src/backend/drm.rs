// Non-Linux: DRM/KMS backend is unavailable; callers must use --winit.
#[cfg(not(target_os = "linux"))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("DRM/KMS backend requires Linux; pass --winit for development on other platforms")
}

// Linux DRM/KMS backend.
// GPU enumeration and rendering are deferred; this skeleton establishes the
// session, socket, and event loop that the full implementation will extend.
#[cfg(target_os = "linux")]
pub fn run() -> anyhow::Result<()> {
    use std::sync::Arc;

    use smithay::{
        backend::{
            session::{libseat::LibSeatSession, Session},
            udev::{UdevBackend, UdevEvent},
        },
        reexports::calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction},
        wayland::socket::ListeningSocketSource,
    };

    use crate::state::{WeftClientState, WeftCompositorState};

    let mut display =
        smithay::reexports::wayland_server::Display::<WeftCompositorState>::new()?;
    let display_handle = display.handle();

    let mut event_loop: EventLoop<'static, WeftCompositorState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    let loop_signal = event_loop.get_signal();

    // Gain DRM device access without root via libseat.
    let (session, _notifier) = LibSeatSession::new()
        .map_err(|e| anyhow::anyhow!("libseat session failed: {e}"))?;

    let listening_socket = ListeningSocketSource::new_auto()
        .map_err(|e| anyhow::anyhow!("Wayland socket creation failed: {e}"))?;
    let socket_name = listening_socket.socket_name().to_os_string();
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);
    tracing::info!(?socket_name, "Wayland compositor socket open");

    loop_handle
        .insert_source(listening_socket, |client_stream, _, state| {
            state
                .display_handle
                .insert_client(client_stream, Arc::new(WeftClientState::default()))
                .unwrap();
        })
        .map_err(|e| anyhow::anyhow!("socket source insertion failed: {e}"))?;

    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // Safety: the display is owned by this Generic source and is never
                // dropped while the event loop runs.
                unsafe {
                    display.get_mut().dispatch_clients(state).unwrap();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("display source insertion failed: {e}"))?;

    // Enumerate GPU nodes via udev; hotplug events arrive through calloop.
    let udev_backend = UdevBackend::new(session.seat())?;
    loop_handle
        .insert_source(udev_backend, move |event, _, _state| match event {
            UdevEvent::Added { device_id, path } => {
                tracing::info!(?device_id, ?path, "GPU device added");
            }
            UdevEvent::Changed { device_id } => {
                tracing::debug!(?device_id, "GPU device changed");
            }
            UdevEvent::Removed { device_id } => {
                tracing::info!(?device_id, "GPU device removed");
            }
        })
        .map_err(|e| anyhow::anyhow!("udev source insertion failed: {e}"))?;

    let mut state = WeftCompositorState::new(
        display_handle,
        loop_signal,
        loop_handle,
        session.seat(),
    );

    tracing::info!("DRM/KMS backend initialised; entering event loop");
    event_loop.run(None, &mut state, |_| {})?;

    Ok(())
}
