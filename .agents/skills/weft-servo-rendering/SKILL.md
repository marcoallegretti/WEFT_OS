---
name: weft-servo-rendering
description: Use for the Servo embedders, painting and presentation, the Servo fork and Stylo overrides, the system UI and application pages.
---

# Servo rendering and the forks

Owners: `weft-servo-shell` renders trusted system chrome; `weft-app-shell`
renders one untrusted application document and its narrow bridge. Neither holds
session lifecycle. Read the blueprint's
[Servo host contract](../../../BLUEPRINT.md#71-servo-host-contract),
[system interface](../../../BLUEPRINT.md#8-system-interface-and-application-behavior)
and [build policy](../../../BLUEPRINT.md#101-one-reproducible-product-build).

## Where things live

| Concern | Source |
|---|---|
| System shell host and event loop | [weft-servo-shell embedder](../../../crates/weft-servo-shell/src/embedder.rs), [main.rs](../../../crates/weft-servo-shell/src/main.rs) |
| Application host, `resolve_weft_app_url` (app ID and `ui.entry` to a `file://` URL), injected bridge | [weft-app-shell embedder](../../../crates/weft-app-shell/src/embedder.rs), [main.rs](../../../crates/weft-app-shell/src/main.rs) |
| Key mapping | [servo-shell keyutils](../../../crates/weft-servo-shell/src/keyutils.rs), [app-shell keyutils](../../../crates/weft-app-shell/src/keyutils.rs) |
| Shell protocol clients | [servo-shell client](../../../crates/weft-servo-shell/src/shell_client.rs), [app-shell client](../../../crates/weft-app-shell/src/shell_client.rs) |
| System UI page and UI kit | [system-ui.html](../../../infra/shell/system-ui.html), [weft-ui-kit.js](../../../infra/shell/weft-ui-kit.js) |
| Pin notes (not authoritative for the selected revision) | [SERVO_PIN.md](../../../crates/weft-servo-shell/SERVO_PIN.md) |

## Selected revisions

The Servo and Stylo revisions are the ones `Cargo.lock` records for the `servo`
and `stylo` packages, not a branch name or `SERVO_PIN.md`. Servo comes from the
`marcoallegretti/servo` fork; Stylo currently comes from upstream `servo/stylo`,
because no root-level override selects the `marcoallegretti/stylo` fork. Inspect
them with:

```sh
grep -A2 'name = "servo"' Cargo.lock
grep -A2 'name = "stylo"' Cargo.lock
```

Read the `servo` crate (`components/servo`), shared embedder input types and
rendering contexts at that revision before using an API. Servo's own `[patch]`
tables do not apply to WEFT: a Stylo override that a Servo-fork feature depends
on must be declared in WEFT's root `Cargo.toml` and confirmed in the resolved
graph. Each fork patch keeps a purpose, upstream status and removal condition.

For cross-repository work, clone the forks next to WEFT and point Cargo at them
with local `[patch]` entries outside the repository. Those overrides rewrite
`Cargo.lock`; never commit that change, and rerun checks without the overrides
before reporting evidence for the committed graph.

## Working rules

- A frame exists only after `WebView::paint` and the backend's present
  operation. Event-loop progress, a READY message or a softbuffer present of an
  unpainted buffer is not a rendered frame. Readback follows painting.
- Handle zero-size and minimized windows, resize, output scale, first frame,
  idle wakeup and shutdown without continuous unbounded repainting.
- Keep the trust distinction: system-control endpoints and credentials never
  reach application documents. Policy for package resources, navigation,
  downloads, file URLs and network requests is enforced in the engine or OS, not
  in page JavaScript.
- Treat manifest names, descriptions and icons as untrusted in the launcher; no
  unsafe HTML injection. Use one endpoint configuration and bridge.
- Extract common host, render and input code only after one working path exists
  in both hosts.

## Evidence

`python .agents/scripts/verify.py servo-embed` type-checks both hosts with the
real renderer. Without `servo-embed` the shells build as stubs; their success is
not rendering evidence. Rendering, input and backdrop or transparency claims need
the deterministic page and pixel checks described in
[integration verification](../weft-integration-verification/SKILL.md).
