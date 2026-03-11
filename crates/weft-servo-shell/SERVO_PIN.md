# Servo Pin

## Current pin

| Field   | Value                                                                              |
|---------|------------------------------------------------------------------------------------|
| Source  | <https://github.com/marcoallegretti/servo> (fork of servo/servo)                  |
| Branch  | `servo-weft`                                                                       |
| Rev     | `04ca254f843ed650d3e5b14e5693ad51a60cc84b` (servo-weft tip, 2026-03-11)           |
| Crate   | `servo` (package name as of 2026-03-11; previously `libservo`)                    |
| Feature | `servo-embed` (optional; off by default)                                           |

## Adding the Cargo dependencies

The Servo deps are **not** in `Cargo.toml` by default to avoid pulling the
Servo monorepo (~1 GB) into every `cargo check` cycle. To activate, add the
following to `crates/weft-servo-shell/Cargo.toml` and change the `servo-embed`
feature line to declare `dep:servo`, `dep:winit`, and `dep:softbuffer`:

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

Then build:

```sh
cargo build -p weft-servo-shell --features servo-embed
```

The first build downloads and compiles Servo and its dependencies, which takes
30–60 minutes cold. Subsequent incremental builds are faster.

## System dependencies

The following system packages are required when `servo-embed` is enabled:

- `libgles2-mesa-dev` or equivalent OpenGL ES headers
- `libssl-dev`
- `libdbus-1-dev`
- `libudev-dev`
- `libxkbcommon-dev`
- `libwayland-dev`

On Fedora/RHEL: `mesa-libGL-devel openssl-devel dbus-devel systemd-devel libxkbcommon-devel wayland-devel`

## Rendering approach

Default: `SoftwareRenderingContext` (CPU rasterisation) blitted to a
`softbuffer`-backed winit window.

EGL path (scaffolded): set `WEFT_EGL_RENDERING=1` at runtime. The embedder
attempts `WindowRenderingContext::new` using the winit display and window
handles. If construction fails it falls back to software automatically.
When the EGL path is active Servo presents directly to the EGL surface;
the softbuffer blit is skipped. Full DMA-BUF export to the Wayland
compositor is not yet wired (`RenderingCtx::Egl` blit body is a no-op).

## Known gaps at this pin

- **GAP-1**: ~~Wayland input events not forwarded to Servo~~ **Resolved** — keyboard and
  mouse events forwarded via `webview.notify_input_event`; key mapping in `keyutils.rs`.
- **GAP-2**: EGL `WindowRenderingContext` path scaffolded (`WEFT_EGL_RENDERING=1`);
  DMA-BUF export to the Wayland compositor (linux-dmabuf-unstable-v1) not yet wired.
- **GAP-3**: WebGPU adapter on Mesa may fail CTS — validation task, requires Mesa GPU hardware.
- **GAP-4**: ~~CSS Grid~~ **Grid resolved** (Taffy-backed, fully wired). CSS `backdrop-filter` is
  unimplemented (servo/servo issue [#41567](https://github.com/servo/servo/issues/41567)).
  Implementation requires two changes:
  1. Enable `backdrop-filter` parsing in `servo/stylo` — the property is disabled at the current
     stylo pin (`dca3934`); requires a `marcoallegretti/stylo` fork with a `servo-weft` branch,
     then patch the stylo dep in this fork's `Cargo.toml` via `[patch."https://github.com/servo/stylo"]`.
  2. Wire the display list in `servo/servo` (this fork): add `backdrop-filter` to
     `establishes_stacking_context` and `establishes_containing_block_for_all_descendants` in
     `components/layout/style_ext.rs`, then call `push_backdrop_filter` in
     `components/layout/display_list/mod.rs`.
- **GAP-5**: Per-app process isolation — requires Servo multi-process (constellation) architecture.

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

4. Run `cargo update` to resolve the new stylo deps.
5. Commit both the `Cargo.toml` and `Cargo.lock` changes to `servo-weft`.
