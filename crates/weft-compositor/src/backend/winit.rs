use std::{sync::Arc, time::Duration};

use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
    utils::{Rectangle, Transform},
    wayland::socket::ListeningSocketSource,
};

use crate::{
    appd_ipc::{self, WeftAppdIpc},
    input,
    state::{WeftClientState, WeftCompositorState},
};

pub fn run() -> anyhow::Result<()> {
    let display = smithay::reexports::wayland_server::Display::<WeftCompositorState>::new()?;
    let display_handle = display.handle();

    let mut event_loop: EventLoop<'static, WeftCompositorState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    let loop_signal = event_loop.get_signal();

    let (mut backend, winit) =
        winit::init().map_err(|e| anyhow::anyhow!("winit init failed: {e}"))?;

    let mode = OutputMode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    let output = Output::new(
        "WEFT-winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "WEFT".to_string(),
            model: "Winit".to_string(),
        },
    );
    let _global = output.create_global::<WeftCompositorState>(&display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    // Create the listening socket; each connecting client is inserted with
    // default per-client data so CompositorHandler::client_compositor_state works.
    let listening_socket = ListeningSocketSource::new_auto()
        .map_err(|e| anyhow::anyhow!("Wayland socket creation failed: {e}"))?;
    let socket_name = listening_socket.socket_name().to_os_string();
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &socket_name) };
    tracing::info!(?socket_name, "Wayland compositor socket open");

    loop_handle
        .insert_source(listening_socket, |client_stream, _, state| {
            if let Err(e) = state
                .display_handle
                .insert_client(client_stream, Arc::new(WeftClientState::default()))
            {
                tracing::warn!(?e, "failed to insert Wayland client");
            }
        })
        .map_err(|e| anyhow::anyhow!("socket source insertion failed: {e}"))?;

    // Register the display fd so calloop dispatches Wayland client messages when readable.
    loop_handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // Safety: the display is owned by this Generic source and is never dropped
                // while the event loop runs.
                unsafe {
                    if let Err(e) = display.get_mut().dispatch_clients(state) {
                        tracing::warn!(?e, "Wayland dispatch error");
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow::anyhow!("display source insertion failed: {e}"))?;

    let mut state = WeftCompositorState::new(
        display_handle,
        loop_signal,
        loop_handle.clone(),
        "seat-0".to_string(),
    );
    state.space.map_output(&output, (0, 0));

    #[cfg(unix)]
    {
        state.appd_ipc = Some(WeftAppdIpc::new(appd_ipc::compositor_socket_path()));
        if let Err(e) = appd_ipc::setup(&mut state) {
            tracing::warn!(?e, "compositor IPC setup failed");
        }
    }

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let start_time = std::time::Instant::now();

    // WinitEventLoop implements calloop's EventSource; Winit events arrive
    // through the same dispatch loop as all other compositor sources.
    loop_handle
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                let new_mode = OutputMode {
                    size,
                    refresh: 60_000,
                };
                output.change_current_state(Some(new_mode), None, None, None);
                output.set_preferred(new_mode);
                state.space.map_output(&output, (0, 0));
                damage_tracker = OutputDamageTracker::from_output(&output);
                state
                    .weft_shell_state
                    .reconfigure_panels(0, 0, size.w, size.h);
                state.weft_shell_state.retain_alive_panels();
            }
            WinitEvent::Input(input_event) => {
                input::process_input_event(state, input_event);
            }
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let full_damage = Rectangle::from_size(size);

                let render_ok = match backend.bind() {
                    Ok((renderer, mut framebuffer)) => smithay::desktop::space::render_output::<
                        _,
                        WaylandSurfaceRenderElement<GlesRenderer>,
                        _,
                        _,
                    >(
                        &output,
                        renderer,
                        &mut framebuffer,
                        1.0,
                        0,
                        [&state.space],
                        &[],
                        &mut damage_tracker,
                        [0.1_f32, 0.1, 0.1, 1.0],
                    )
                    .map_err(|e| tracing::warn!(?e, "render_output failed"))
                    .is_ok(),
                    Err(e) => {
                        tracing::warn!(?e, "backend bind failed");
                        false
                    }
                };
                if render_ok {
                    if let Err(e) = backend.submit(Some(&[full_damage])) {
                        tracing::warn!(?e, "backend submit failed");
                    }
                }

                state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                });

                state.space.refresh();
                state.popups.cleanup();
                let _ = state.display_handle.flush_clients();

                // Request next redraw to drive continuous rendering.
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => {
                state.running = false;
                state.loop_signal.stop();
            }
            _ => (),
        })
        .map_err(|e| anyhow::anyhow!("winit source insertion failed: {e}"))?;

    event_loop.run(None, &mut state, |_| {})?;

    Ok(())
}
