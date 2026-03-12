# Servo Pin

## Current pin

| Field   | Value                                                                              |
|---------|------------------------------------------------------------------------------------|
| Source  | <https://github.com/marcoallegretti/servo> (fork of servo/servo)                  |
| Branch  | `servo-weft`                                                                       |
| Rev     | `04ca254f843ed650d3e5b14e5693ad51a60cc84b` (servo-weft tip, 2026-03-11)           |
| Crate   | `servo` (package name as of 2026-03-11; previously `libservo`)                    |
| Feature | `servo-embed` (optional; off by default)                                           |

## Cargo dependencies

The Servo deps are wired in `crates/weft-servo-shell/Cargo.toml` and
`crates/weft-app-shell/Cargo.toml` under the `servo-embed` optional feature.
They are off by default to avoid pulling the Servo monorepo (~1 GB) into every
`cargo check` cycle.

Current `Cargo.toml` block (already committed):

```toml
[features]
servo-embed = ["dep:servo", "dep:winit", "dep:softbuffer"]

[dependencies.servo]
git = "https://github.com/marcoallegretti/servo"
branch = "servo-weft"
optional = true
default-features = false

[dependencies.winit]
version = "0.30"
optional = true
features = ["wayland"]

[dependencies.softbuffer]
version = "0.4"
optional = true
```

To build:

```sh
cargo build -p weft-servo-shell --features servo-embed
```

The first build downloads and compiles Servo and its dependencies, which takes
30–60 minutes cold. Subsequent incremental builds are faster.

## System dependencies

The following system packages are required when `servo-embed` is enabled:

- `Mesa-libGLES-devel`
- `libopenssl-devel`
- `dbus-1-devel`
- `libudev-devel`
- `libxkbcommon-devel`
- `libwayland-devel`

Install with: `sudo zypper install -y Mesa-libGLES-devel libopenssl-devel dbus-1-devel libudev-devel libxkbcommon-devel libwayland-devel`

## Rendering approach

Default: `SoftwareRenderingContext` (CPU rasterisation) blitted to a
`softbuffer`-backed winit window.

EGL path: set `WEFT_EGL_RENDERING=1` at runtime. The embedder attempts
`WindowRenderingContext::new` using the winit display and window handles.
If construction fails it falls back to software automatically.
When the EGL path is active Servo presents directly to the EGL surface via
surfman's `eglSwapBuffers`; the softbuffer blit is skipped. Mesa handles
DMA-BUF buffer sharing with the compositor transparently.

## Known limitations at this pin

- **Wayland input events**: ~~not forwarded to Servo~~ **Resolved** — keyboard and
  mouse events forwarded via `webview.notify_input_event`; key mapping in `keyutils.rs`.
- **Wayland surface sharing**: ~~`ZweftShellWindowV1` created with `surface = null`~~ **Resolved** —
  `ShellClient::connect_with_display(display_ptr, surface_ptr)` uses
  `Backend::from_foreign_display` to share winit's `wl_display` connection; the winit
  `wl_surface` pointer is passed directly to `create_window`, associating the compositor
  shell slot with the actual rendered surface. `ShellClient` is now constructed inside
  `resumed()` after the winit window exists, not before the event loop. EGL path and
  per-frame event dispatch are unchanged.
  Protocol note: `wayland-scanner 0.31` generates `_type` (not `r#type`) for the
  `navigation_gesture` event arg named `type`.
- **WebGPU on Mesa**: adapter may fail CTS — validation task, requires Mesa GPU hardware.
- **CSS layout features**: ~~CSS Grid~~ **Grid resolved** (Taffy-backed, fully wired).
  ~~CSS `backdrop-filter` unimplemented~~ **`backdrop-filter` resolved** (servo/servo issue
  [#41567](https://github.com/servo/servo/issues/41567)). Implemented across two commits:
  - `marcoallegretti/stylo` `servo-weft` `f1ba496`: removed `servo_pref = "layout.unimplemented"`
    from `backdrop-filter` in `style/properties/longhands.toml` (enables parsing).
  - `marcoallegretti/servo` `servo-weft` `8e7dc40`: `Cargo.toml` patched to use the stylo fork;
    `style_ext.rs` adds `backdrop-filter` to `establishes_stacking_context` and
    `establishes_containing_block_for_all_descendants`; `stacking_context.rs` guards the
    WebRender stacking-context early-return on `backdrop_filter.0.is_empty()`; `display_list/mod.rs`
    adds `BuilderForBoxFragment::build_backdrop_filter` calling `push_backdrop_filter` before
    background paint.
- **Per-app process isolation**: ~~not implemented~~ **Resolved** — each app runs in a separate
  `weft-app-shell` and `weft-runtime` OS process pair supervised by `weft-appd`. OS-level
  isolation does not require Servo's multi-process constellation architecture.
- **SpiderMonkey sandbox**: SpiderMonkey is not sandbox-isolated beyond process-level isolation.
  JIT-compiled JS runs with the same memory permissions as the Servo process. WEFT relies on
  SpiderMonkey's own security properties for the JavaScript execution boundary. See
  `docs/security.md` for the full bounded statement. Not addressed.

## Update policy

The `servo-weft` branch is the working branch for WEFT-specific Servo patches.
Upstream servo/servo changes are merged into it periodically.

To rebase onto a new upstream commit:

1. In the `marcoallegretti/servo` repo: `git fetch upstream && git rebase upstream/main` on `servo-weft`.
2. Force-push `servo-weft`.
3. Update `Rev` in this file to the new tip SHA.
4. Run `cargo update -p servo` in the WEFT OS workspace.
5. Confirm the compositor and shell tests still pass.

To submit patches upstream: open a PR from `servo-weft` (or a topic branch) to `servo/servo`.

## Stylo fork

The Stylo CSS engine is a separate repo (`servo/stylo`) pinned at rev `dca3934667dae76c49bb579b268c5eb142d09c6a`
in `Cargo.toml`. To patch it for WEFT-specific changes (e.g. enabling `backdrop-filter`):

1. Fork `servo/stylo` to `marcoallegretti/stylo`, create branch `servo-weft`.
2. Make changes on that branch.
3. Add to this fork's `Cargo.toml` workspace `[patch]` section:

```toml
[patch."https://github.com/servo/stylo"]
stylo = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
stylo_atoms = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
stylo_dom = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
stylo_malloc_size_of = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
stylo_static_prefs = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
stylo_traits = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
selectors = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
servo_arc = { git = "https://github.com/marcoallegretti/stylo", branch = "servo-weft" }
```

1. Run `cargo update` to resolve the new stylo deps.
2. Commit both the `Cargo.toml` and `Cargo.lock` changes to `servo-weft`.
