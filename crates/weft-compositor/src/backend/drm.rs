// Non-Linux: DRM/KMS backend is unavailable; callers must use --winit.
#[cfg(not(target_os = "linux"))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("DRM/KMS backend requires Linux; pass --winit for development on other platforms")
}

// Linux DRM/KMS backend.
#[cfg(target_os = "linux")]
pub fn run() -> anyhow::Result<()> {
    use std::time::Duration;

    use smithay::{
        backend::{
            allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            drm::{DrmDevice, DrmDeviceFd, DrmNode, NodeType},
            egl::EGLDevice,
            libinput::{LibinputInputBackend, LibinputSessionInterface},
            renderer::{
                damage::OutputDamageTracker,
                gles::GlesRenderer,
                multigpu::{gbm::GbmGlesBackend, GpuManager, MultiRenderer},
            },
            session::{
                libseat::{LibSeatSession, LibSeatSessionNotifier},
                Session,
            },
            udev::{UdevBackend, UdevEvent},
        },
        desktop::{space::space_render_elements, Space, Window},
        output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
        reexports::{
            calloop::{
                timer::{TimeoutAction, Timer},
                EventLoop, Interest, Mode, PostAction,
            },
            wayland_server::Display,
        },
        utils::Transform,
    };

    use crate::{input, state::WeftCompositorState};

    let mut display: Display<WeftCompositorState> = Display::new()?;
    let display_handle = display.handle();

    let mut event_loop: EventLoop<'static, WeftCompositorState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    let loop_signal = event_loop.get_signal();

    // Open a libseat session to gain DRM device access without root.
    let (session, notifier) = LibSeatSession::new()
        .map_err(|e| anyhow::anyhow!("libseat session failed: {e}"))?;

    // Discover GPU nodes via udev.
    let udev_backend = UdevBackend::new(session.seat())?;

    let mut state = WeftCompositorState::new(
        display_handle,
        loop_signal.clone(),
        loop_handle.clone(),
        session.seat(),
    );

    // Register the udev backend with calloop so device hotplug is handled.
    loop_handle.insert_source(udev_backend, {
        let signal = loop_signal.clone();
        move |event, _, _state| match event {
            UdevEvent::Added { device_id, path } => {
                tracing::info!(?device_id, ?path, "GPU device added");
            }
            UdevEvent::Changed { device_id } => {
                tracing::debug!(?device_id, "GPU device changed");
            }
            UdevEvent::Removed { device_id } => {
                tracing::info!(?device_id, "GPU device removed");
                signal.stop();
            }
        }
    })?;

    tracing::info!("DRM/KMS backend initialised; entering event loop");

    loop {
        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
        event_loop.dispatch(Some(Duration::from_millis(16)), &mut state)?;

        if loop_signal.is_stopped() {
            break;
        }
    }

    Ok(())
}
