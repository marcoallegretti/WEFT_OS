---
name: weft-compositor-input
description: Use for the Smithay compositor, Wayland protocols, surfaces, focus, geometry, input routing and the DRM and winit backends.
---

# Compositor, Wayland and input

Owner: `weft-compositor` owns surfaces, accepted geometry, focus, stacking, work
areas, input routing and presentation. Appd owns session identity; the shells
request and render. Read the blueprint's
[surface and window contract](../../../BLUEPRINT.md#72-surface-and-window-contract),
[input requirements](../../../BLUEPRINT.md#73-required-input-and-accessibility-behavior)
and [backend completion](../../../BLUEPRINT.md#74-backend-specific-completion).

## Where things live

| Concern | Source |
|---|---|
| Wayland globals and handlers (xdg-shell, layer-shell, DMA-BUF state, presentation) | [state.rs](../../../crates/weft-compositor/src/state.rs) |
| Keyboard, pointer and wheel routing | [input.rs](../../../crates/weft-compositor/src/input.rs) |
| `zweft-shell-unstable-v1` server side | [protocols/mod.rs](../../../crates/weft-compositor/src/protocols/mod.rs), [protocol XML](../../../protocol/weft-shell-unstable-v1.xml) |
| Appd association channel | [appd_ipc.rs](../../../crates/weft-compositor/src/appd_ipc.rs), [weft-ipc-types](../../../crates/weft-ipc-types/src/lib.rs) |
| Nested backend | [backend/winit.rs](../../../crates/weft-compositor/src/backend/winit.rs) |
| DRM/KMS, udev, libinput, session | [backend/drm.rs](../../../crates/weft-compositor/src/backend/drm.rs), [backend/drm_device.rs](../../../crates/weft-compositor/src/backend/drm_device.rs) |

## Working rules

- Verify Smithay behavior in the `smithay 0.7` source selected by `Cargo.lock`
  (output frame submission, DMA-BUF global creation, seat and focus handling),
  not in newer upstream examples. Advertise only globals the compositor actually
  serves.
- Bind surfaces to sessions through trusted launch or connection evidence and
  observed client identity, with opaque IDs and generations. A PID, title or
  custom shell-protocol request is not authority. Only the trusted shell acquires
  privileged roles such as `panel`.
- Effective xdg geometry and acknowledged configure state must agree; echoing a
  `set_geometry` request does not move a surface.
- Prevent stuck keys and buttons after focus loss, client exit, drag
  cancellation and output changes. Keep logical and physical coordinates
  consistent across scale changes.
- DRM paths need the frame completion transition, presentation feedback,
  idle and empty-frame scheduling that resumes on commits, session pause and
  activation, and error recovery. Do not replace existing layer rendering on a
  false premise.

## Evidence

`python .agents/scripts/verify.py portable linux` covers compilation, lints and
the protocol unit tests in `protocols/mod.rs`. It does not run a compositor.
Geometry, focus, stacking, input and presentation claims need a nested winit
session with real clients, and DRM claims need a DRM session on the declared
target; see [integration verification](../weft-integration-verification/SKILL.md).
The compositor and both shells generate their protocol code from the same XML,
so a protocol change affects all three; check them with the `linux` and
`servo-embed` profiles and keep the interface version deliberate.
