# Servo Pin

## Current pin

| Field   | Value                                                                    |
|---------|--------------------------------------------------------------------------|
| Source  | <https://github.com/servo/servo>                                         |
| Rev     | `c242860f0ef4e7c6e60dfea29310167898e6eb38` (main, 2026-03-11)            |
| Crate   | `servo` (package name as of 2026-03-11; previously `libservo`)           |
| Feature | `servo-embed` (optional; off by default)                                 |

## Adding the Cargo dependencies

The Servo deps are **not** in `Cargo.toml` by default to avoid pulling the
Servo monorepo (~1 GB) into every `cargo check` cycle. To activate, add the
following to `crates/weft-servo-shell/Cargo.toml` and change the `servo-embed`
feature line to declare `dep:servo`, `dep:winit`, and `dep:softbuffer`:

```toml
[features]
servo-embed = ["dep:servo", "dep:winit", "dep:softbuffer"]

[dependencies.servo]
git = "https://github.com/servo/servo"
rev = "c242860f0ef4e7c6e60dfea29310167898e6eb38"
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

Pin is reviewed monthly. To update:

1. Check the [Servo release page](https://github.com/servo/servo/releases) for new tags.
2. Update `tag` in `Cargo.toml` and run `cargo update -p servo`.
3. Confirm the compositor and shell tests still pass.
4. Update this file with the new tag and any new or resolved gaps.
