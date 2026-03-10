use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureHoldBeginEvent,
        GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, InputBackend,
        InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionAbsoluteEvent, PointerMotionEvent, TouchCancelEvent, TouchDownEvent,
        TouchFrameEvent, TouchMotionEvent, TouchUpEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
};

use crate::state::WeftCompositorState;

pub fn process_input_event<B: InputBackend>(state: &mut WeftCompositorState, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => handle_keyboard(state, event),
        InputEvent::PointerMotion { event } => handle_pointer_motion(state, event),
        InputEvent::PointerMotionAbsolute { event } => handle_pointer_motion_absolute(state, event),
        InputEvent::PointerButton { event } => handle_pointer_button(state, event),
        InputEvent::PointerAxis { event } => handle_pointer_axis(state, event),
        InputEvent::TouchDown { event } => handle_touch_down(state, event),
        InputEvent::TouchUp { event } => handle_touch_up(state, event),
        InputEvent::TouchMotion { event } => handle_touch_motion(state, event),
        InputEvent::TouchFrame { event } => handle_touch_frame(state, event),
        InputEvent::TouchCancel { event } => handle_touch_cancel(state, event),
        InputEvent::GestureSwipeBegin { event } => handle_gesture_swipe_begin(state, event),
        InputEvent::GestureSwipeUpdate { event } => handle_gesture_swipe_update(state, event),
        InputEvent::GestureSwipeEnd { event } => handle_gesture_swipe_end(state, event),
        InputEvent::GesturePinchBegin { event } => handle_gesture_pinch_begin(state, event),
        InputEvent::GesturePinchUpdate { event } => handle_gesture_pinch_update(state, event),
        InputEvent::GesturePinchEnd { event } => handle_gesture_pinch_end(state, event),
        InputEvent::GestureHoldBegin { event } => handle_gesture_hold_begin(state, event),
        InputEvent::GestureHoldEnd { event } => handle_gesture_hold_end(state, event),
        // Device added/removed events are handled at the backend level.
        InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } => {}
        _ => {}
    }
}

fn handle_keyboard<B: InputBackend>(state: &mut WeftCompositorState, event: B::KeyboardKeyEvent) {
    let serial = SERIAL_COUNTER.next_serial();
    let time = event.time_msec();
    let key_state = event.state();

    if let Some(keyboard) = state.seat.get_keyboard() {
        keyboard.input::<(), _>(
            state,
            event.key_code(),
            key_state,
            serial,
            time,
            |_state, _mods, _keysym| FilterResult::Forward,
        );
    }
}

fn handle_pointer_motion<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::PointerMotionEvent,
) {
    let delta = event.delta();
    state.pointer_location += delta;
    clamp_pointer_to_output_space(state);

    let serial = SERIAL_COUNTER.next_serial();
    let pointer_location = state.pointer_location;
    let under = surface_under(state, pointer_location);

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.motion(
            state,
            under,
            &MotionEvent {
                location: pointer_location,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(state);
    }
}

fn handle_pointer_motion_absolute<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::PointerMotionAbsoluteEvent,
) {
    let output = state.space.outputs().next().cloned();
    if let Some(output) = output {
        let output_geo = state.space.output_geometry(&output).unwrap_or_default();
        let pos = event.position_transformed(output_geo.size);
        state.pointer_location = output_geo.loc.to_f64() + pos;
    }

    let serial = SERIAL_COUNTER.next_serial();
    let pointer_location = state.pointer_location;
    let under = surface_under(state, pointer_location);

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.motion(
            state,
            under,
            &MotionEvent {
                location: pointer_location,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(state);
    }
}

fn handle_pointer_button<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::PointerButtonEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    let button = event.button_code();
    let button_state = event.state();

    // On press: focus the surface under the pointer.
    if button_state == ButtonState::Pressed {
        let pointer_location = state.pointer_location;
        if let Some((surface, _loc)) = surface_under(state, pointer_location) {
            if let Some(keyboard) = state.seat.get_keyboard() {
                keyboard.set_focus(state, Some(surface), serial);
            }
        } else if let Some(keyboard) = state.seat.get_keyboard() {
            keyboard.set_focus(state, None, serial);
        }
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.button(
            state,
            &ButtonEvent {
                button,
                state: button_state,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(state);
    }
}

fn handle_pointer_axis<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::PointerAxisEvent,
) {
    let horizontal = event.amount(Axis::Horizontal);
    let vertical = event.amount(Axis::Vertical);
    let h_discrete = event.amount_v120(Axis::Horizontal);
    let v_discrete = event.amount_v120(Axis::Vertical);
    let source = event.source();

    let mut frame = AxisFrame::new(event.time_msec()).source(source);

    if let Some(v) = horizontal {
        if v != 0.0 {
            frame = frame.value(Axis::Horizontal, v);
        }
        if let Some(d) = h_discrete {
            frame = frame.v120(Axis::Horizontal, d as i32);
        }
    }
    if let Some(v) = vertical {
        if v != 0.0 {
            frame = frame.value(Axis::Vertical, v);
        }
        if let Some(d) = v_discrete {
            frame = frame.v120(Axis::Vertical, d as i32);
        }
    }

    if source == AxisSource::Finger {
        if event.amount(Axis::Horizontal).unwrap_or(0.0) == 0.0 {
            frame = frame.stop(Axis::Horizontal);
        }
        if event.amount(Axis::Vertical).unwrap_or(0.0) == 0.0 {
            frame = frame.stop(Axis::Vertical);
        }
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.axis(state, frame);
        pointer.frame(state);
    }
}

fn handle_touch_down<B: InputBackend>(state: &mut WeftCompositorState, event: B::TouchDownEvent) {
    let serial = SERIAL_COUNTER.next_serial();
    let output = state.space.outputs().next().cloned();
    if let Some(output) = output {
        let output_geo = state.space.output_geometry(&output).unwrap_or_default();
        let pos = event.position_transformed(output_geo.size);
        let location = output_geo.loc.to_f64() + pos;
        let under = surface_under(state, location);

        if let Some(touch) = state.seat.get_touch() {
            touch.down(
                state,
                under,
                &smithay::input::touch::DownEvent {
                    slot: event.slot(),
                    location,
                    serial,
                    time: event.time_msec(),
                },
            );
        }
    }
}

fn handle_touch_up<B: InputBackend>(state: &mut WeftCompositorState, event: B::TouchUpEvent) {
    let serial = SERIAL_COUNTER.next_serial();
    if let Some(touch) = state.seat.get_touch() {
        touch.up(
            state,
            &smithay::input::touch::UpEvent {
                slot: event.slot(),
                serial,
                time: event.time_msec(),
            },
        );
    }
}

fn handle_touch_motion<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::TouchMotionEvent,
) {
    let output = state.space.outputs().next().cloned();
    if let Some(output) = output {
        let output_geo = state.space.output_geometry(&output).unwrap_or_default();
        let pos = event.position_transformed(output_geo.size);
        let location = output_geo.loc.to_f64() + pos;
        let under = surface_under(state, location);

        if let Some(touch) = state.seat.get_touch() {
            touch.motion(
                state,
                under,
                &smithay::input::touch::MotionEvent {
                    slot: event.slot(),
                    location,
                    time: event.time_msec(),
                },
            );
        }
    }
}

fn handle_touch_frame<B: InputBackend>(
    state: &mut WeftCompositorState,
    _event: B::TouchFrameEvent,
) {
    if let Some(touch) = state.seat.get_touch() {
        touch.frame(state);
    }
}

fn handle_touch_cancel<B: InputBackend>(
    state: &mut WeftCompositorState,
    _event: B::TouchCancelEvent,
) {
    if let Some(touch) = state.seat.get_touch() {
        touch.cancel(state);
    }
}

fn handle_gesture_swipe_begin<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GestureSwipeBeginEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_swipe_begin(
            state,
            &smithay::input::pointer::GestureSwipeBeginEvent {
                serial,
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }
}

fn handle_gesture_swipe_update<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GestureSwipeUpdateEvent,
) {
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_swipe_update(
            state,
            &smithay::input::pointer::GestureSwipeUpdateEvent {
                time: event.time_msec(),
                delta: event.delta(),
            },
        );
    }
}

fn handle_gesture_swipe_end<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GestureSwipeEndEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_swipe_end(
            state,
            &smithay::input::pointer::GestureSwipeEndEvent {
                serial,
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }
}

fn handle_gesture_pinch_begin<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GesturePinchBeginEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_pinch_begin(
            state,
            &smithay::input::pointer::GesturePinchBeginEvent {
                serial,
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }
}

fn handle_gesture_pinch_update<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GesturePinchUpdateEvent,
) {
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_pinch_update(
            state,
            &smithay::input::pointer::GesturePinchUpdateEvent {
                time: event.time_msec(),
                delta: event.delta(),
                scale: event.scale(),
                rotation: event.rotation(),
            },
        );
    }
}

fn handle_gesture_pinch_end<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GesturePinchEndEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_pinch_end(
            state,
            &smithay::input::pointer::GesturePinchEndEvent {
                serial,
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }
}

fn handle_gesture_hold_begin<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GestureHoldBeginEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_hold_begin(
            state,
            &smithay::input::pointer::GestureHoldBeginEvent {
                serial,
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }
}

fn handle_gesture_hold_end<B: InputBackend>(
    state: &mut WeftCompositorState,
    event: B::GestureHoldEndEvent,
) {
    let serial = SERIAL_COUNTER.next_serial();
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_hold_end(
            state,
            &smithay::input::pointer::GestureHoldEndEvent {
                serial,
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }
}

/// Returns the surface and its local coordinates under the given position.
pub fn surface_under(
    state: &WeftCompositorState,
    point: Point<f64, Logical>,
) -> Option<(
    smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    Point<f64, Logical>,
)> {
    state.space.element_under(point).and_then(|(window, loc)| {
        window
            .surface_under(
                point - loc.to_f64(),
                smithay::desktop::WindowSurfaceType::ALL,
            )
            .map(|(surface, surface_loc)| (surface, (loc.to_f64() + surface_loc.to_f64())))
    })
}

fn clamp_pointer_to_output_space(state: &mut WeftCompositorState) {
    let bbox = state
        .space
        .outputs()
        .filter_map(|o| state.space.output_geometry(o))
        .fold(
            smithay::utils::Rectangle::<i32, Logical>::default(),
            |acc, r| acc.merge(r),
        );
    if bbox.size.w > 0 && bbox.size.h > 0 {
        state.pointer_location.x = state
            .pointer_location
            .x
            .clamp(bbox.loc.x as f64, (bbox.loc.x + bbox.size.w - 1) as f64);
        state.pointer_location.y = state
            .pointer_location
            .y
            .clamp(bbox.loc.y as f64, (bbox.loc.y + bbox.size.h - 1) as f64);
    }
}
