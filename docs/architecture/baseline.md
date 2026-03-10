# WEFT OS Architecture Baseline

This document records the implementation baseline derived from the authoritative blueprint in `docu_dev/WEFT-OS-COMPREHENSIVE-BLUEPRINT.md`.

## Authority

The comprehensive blueprint is the source of truth for technical direction in this repository.

The earlier concept and integrative blueprint documents are secondary references. They are useful only where they do not conflict with the comprehensive blueprint.

## Defined foundation

The current defined foundation is:

- Linux kernel
- systemd for the initial prototype
- Smithay for the Wayland compositor
- Servo as the system shell renderer running as a Wayland client
- Wasmtime for application execution
- Rust for all core system components

## System shape

The intended top-level structure is:

```text
Linux kernel
  -> weft-compositor
  -> servo-shell
  -> weft-appd
```

`weft-compositor` owns Wayland surfaces, surface stacking, input routing, and compositing.

`servo-shell` renders the system shell UI from HTML and CSS as a Wayland client.

`weft-appd` is the process supervisor and capability broker for application launch and lifecycle management.

## Open blockers

The following items are not treated as solved in this repository:

- `weft-shell-protocol` specification
- Wasm–Servo channel design
- Servo Wayland input audit through winit
- per-app Servo isolation model
- SpiderMonkey stability verification relevant to UI-side JavaScript

## Consequences for repository scope

Until the open blockers are closed at design level, this repository should avoid:

- broad multi-crate scaffolding for unresolved runtime boundaries
- developer SDK or packaging tooling built against unstable contracts
- application examples that imply the Wasm–Servo contract is already decided

## Historical mismatches that are not implementation authority

The earlier concept document is not used as implementation truth where it conflicts with the comprehensive blueprint.

Examples of mismatches already identified:

- unsigned package examples versus the authoritative signed bundle requirement
- simplified permission names versus the authoritative capability taxonomy
- a historical top-level `shaders/` directory versus the authoritative package layout
- broader early-language discussion that does not define current core-system policy
