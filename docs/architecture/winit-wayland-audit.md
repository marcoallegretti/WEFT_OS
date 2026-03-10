# winit Wayland Input Audit

Audit of winit's Wayland backend against WEFT OS system shell input requirements.
Required by Wave 4 gate: Servo Wayland input audit result assessed.

Source: blueprint Section 11, GAP 1. Servo version audited: main branch (2025).
winit version audited: 0.30.x (smithay-client-toolkit backend).

---

## Audit Scope

WEFT requires correct and reliable keyboard, mouse, touch, and IME input for
the system shell. Input regressions are system-level failures because there is
no fallback input path.

Servo delegates all windowing and input to winit. winit's Wayland backend uses
smithay-client-toolkit (sctk) as the protocol implementation layer.

---

## Findings

### Keyboard input

**Status: FUNCTIONAL with known limitation**

Basic key events, modifiers, and repeat work correctly via xkb.
The `xdg_keyboard_shortcuts_inhibit` protocol is not implemented in winit's
Wayland backend, so system keyboard shortcuts (e.g. Alt+F4) cannot be
inhibited by client surfaces. This affects the system shell if it needs to
handle those key combinations before the compositor does.

Relevant winit issue: https://github.com/rust-windowing/winit/issues/2787 (open).

### Pointer input

**Status: FUNCTIONAL**

Button, scroll, and motion events work correctly. `zwp_relative_pointer_v1`
(relative motion for pointer locking) is implemented. `zwp_pointer_constraints_v1`
(locked/confined pointer) is implemented in winit 0.30+.
Frame-accurate pointer position via `wl_pointer.frame` is handled.

### Touch input

**Status: PARTIAL**

Single-touch is functional. Multi-touch slots are tracked via `wl_touch` protocol.
Gesture recognition is not implemented in winit — gestures from the compositor
(`zwp_pointer_gestures_v1`) are not consumed. This affects swipe/pinch gesture
handling in the system shell.

Relevant winit issue: not filed as of audit date.

### IME (Input Method Editor)

**Status: INCOMPLETE**

`zwp_text_input_v3` is implemented in sctk 0.18+ but winit's integration is
incomplete. Specifically:
- Pre-edit text display is not forwarded to the application's IME event stream
  in all cases.
- `done` events with surrounding text are not always handled correctly.

This means CJK and other IME-dependent input in the system shell HTML will not
work correctly.

Relevant sctk issue: https://github.com/Smithay/client-toolkit/issues/605 (open).

### Frame pacing (vsync alignment)

**Status: NOT IMPLEMENTED**

winit does not implement `wp_presentation_time` (the Wayland presentation
feedback protocol). Frame timing is based on `wl_callback` only. This means
Servo cannot align rendering to compositor vsync, causing frame pacing issues
on variable-refresh-rate displays and tearing on fixed-refresh displays.

This must be fixed before the system shell is suitable for production use.
Relevant Servo issue: not filed as of audit date.

### DMA-BUF surface sharing

**Status: UNVERIFIED**

The Servo → WebRender → wgpu → wl_surface pipeline on Wayland may or may not
use `zwp_linux_dmabuf_v1` for zero-copy buffer sharing. This audit did not
test it under the WEFT compositor (requires QEMU or real hardware).
Must be verified when DRM backend testing is available.

---

## Assessment

| Input area          | Status      | Blocks Wave 4 skeleton? |
|---------------------|-------------|-------------------------|
| Keyboard (basic)    | Functional  | No                      |
| Keyboard shortcuts  | Gap         | No (deferred)           |
| Pointer             | Functional  | No                      |
| Touch (single)      | Functional  | No                      |
| Touch (gestures)    | Gap         | No (deferred)           |
| IME                 | Incomplete  | No (system shell uses minimal JS) |
| Frame pacing        | Not impl.   | No (deferred, required before GA) |
| DMA-BUF             | Unverified  | No (requires hardware test) |

None of the identified gaps block the Wave 4 skeleton or initial integration.
They block production readiness, as documented in the blueprint.

**Gate decision**: Wave 4 may proceed. The gaps above are tracked as known
work items, not blocking conditions for skeleton implementation.

---

## Required Follow-up

Before WEFT OS reaches GA:

1. Contribute `wp_presentation_time` support to winit (or contribute to Servo
   to work around it via the compositor's presentation feedback).
2. Contribute `zwp_text_input_v3` fix to sctk and winit for correct IME.
3. File and track a winit issue for `zwp_pointer_gestures_v1`.
4. Verify DMA-BUF path under the WEFT DRM compositor (requires hardware).
5. File issues for all confirmed gaps in the Servo and winit issue trackers
   per the blueprint contribution workflow.
