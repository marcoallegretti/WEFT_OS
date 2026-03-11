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
- **GAP-3**: WebGPU adapter on Mesa may fail CTS
- **GAP-4**: CSS `backdrop-filter` and CSS Grid have partial coverage

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
