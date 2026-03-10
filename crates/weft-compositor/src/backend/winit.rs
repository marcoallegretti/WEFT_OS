use std::time::Duration;

use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitEventLoop, WinitGraphicsBackend},
    },
    desktop::{space::space_render_elements, Space, Window},
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{calloop::EventLoop, wayland_server::Display},
    utils::Transform,
};

use crate::{input, state::WeftCompositorState};

pub fn run() -> anyhow::Result<()> {
    let mut display: Display<WeftCompositorState> = Display::new()?;
    let display_handle = display.handle();

    let mut event_loop: EventLoop<'static, WeftCompositorState> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();
    let loop_signal = event_loop.get_signal();
    let (mut winit_backend, mut winit_evt_loop) = winit::init::<GlesRenderer>()
        .map_err(|e| anyhow::anyhow!("winit backend init failed: {e}"))?;

    let initial_size = winit_backend.window_size();
    let output = Output::new(
        "WEFT-winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "WEFT".to_string(),
            model: "Winit".to_string(),
        },
    );
    let _wl_output_global = output.create_global::<WeftCompositorState>(&display_handle);

    let initial_mode = OutputMode {
        size: initial_size,
        refresh: 60_000,
    };
    output.change_current_state(
        Some(initial_mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(initial_mode);

    let mut state = WeftCompositorState::new(
        display_handle,
        loop_signal,
        loop_handle,
        "seat-0".to_string(),
    );
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let start = std::time::Instant::now();

    loop {
        let dispatch_result = dispatch_winit_events(
            &mut winit_evt_loop,
            &mut state,
            &output,
            &mut damage_tracker,
        );

        if dispatch_result.is_err() || !state.running {
            break;
        }

        display.dispatch_clients(&mut state)?;

        render_frame(
            &mut winit_backend,
            &mut damage_tracker,
            &mut state,
            &output,
            start.elapsed(),
        )?;

        display.flush_clients()?;

        // Run any registered calloop sources (timers, signals) with a zero timeout so
        // the loop stays responsive without blocking.
        event_loop.dispatch(Some(Duration::ZERO), &mut state)?;
    }

    Ok(())
}

fn dispatch_winit_events(
    evt_loop: &mut WinitEventLoop,
    state: &mut WeftCompositorState,
    output: &Output,
    damage_tracker: &mut OutputDamageTracker,
) -> Result<(), ()> {
    evt_loop
        .dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, scale_factor } => {
                let new_mode = OutputMode {
                    size,
                    refresh: 60_000,
                };
                output.change_current_state(
                    Some(new_mode),
                    None,
                    Some(Scale::Fractional(scale_factor)),
                    None,
                );
                output.set_preferred(new_mode);
                state.space.map_output(output, (0, 0));
                *damage_tracker = OutputDamageTracker::from_output(output);
            }
            WinitEvent::Input(input_event) => {
                input::process_input_event(state, input_event);
            }
            WinitEvent::Focus(_focused) => {}
            WinitEvent::Refresh => {}
            WinitEvent::CloseRequested => {
                state.running = false;
            }
        })
        .map_err(|_| ())
}

fn render_frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: &mut OutputDamageTracker,
    state: &mut WeftCompositorState,
    output: &Output,
    elapsed: Duration,
) -> anyhow::Result<()> {
    backend
        .bind()
        .map_err(|e| anyhow::anyhow!("framebuffer bind failed: {e}"))?;

    let age = backend.buffer_age().unwrap_or(0);
    let renderer = backend.renderer();

    let elements =
        space_render_elements::<GlesRenderer, Window, &Space<Window>>(
            renderer,
            [&state.space],
            output,
            1.0_f64,
        )
        .map_err(|e| anyhow::anyhow!("render element collection failed: {e}"))?;

    let result = damage_tracker
        .render_output(renderer, age, &elements, [0.1_f32, 0.1, 0.1, 1.0])
        .map_err(|e| anyhow::anyhow!("render_output failed: {e}"))?;

    backend
        .submit(result.damage.as_deref())
        .map_err(|e| anyhow::anyhow!("buffer submit failed: {e}"))?;

    // Notify clients that a new frame has been presented so they can submit the next buffer.
    for window in state.space.elements() {
        window.send_frame(
            output,
            elapsed,
            Some(Duration::from_secs(1) / 60),
            |_, _| Some(output.clone()),
        );
    }

    Ok(())
}
