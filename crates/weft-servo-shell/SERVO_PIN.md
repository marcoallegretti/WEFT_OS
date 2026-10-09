# Servo Pin

## Current pin

The selected revisions are the ones recorded in the workspace `Cargo.lock`.

| Dependency | Source | Revision | Selected by |
|------------|--------|----------|-------------|
| Servo (`servo` crate) | <https://github.com/marcoallegretti/servo> | `f0bb1aaf981b65c8dddc848bb647045d4c777d11` | `rev` in both shell crates' `Cargo.toml` |
| Stylo (`stylo`, `selectors`, `servo_arc`, ...) | <https://github.com/marcoallegretti/stylo> | `f1ba4969536d63f2e319a561e466c03e78b4a076` | `[patch."https://github.com/servo/stylo"]` in the root `Cargo.toml` |

The Servo revision is `servo-weft` commit `8e7dc40` (backdrop-filter rendering)
plus `f0bb1aa`, which fixes that commit's `servo-layout` build errors. It lives
on the fork branch `fix/backdrop-filter-build-errors` until it is merged into
`servo-weft`; keep that branch until then so the revision stays fetchable.

Servo's own `[patch]` table for Stylo does not apply to WEFT, because Cargo
only honours patches in the root workspace. Without the root override, WEFT
resolves upstream Stylo `dca3934`, where `backdrop-filter` is still gated
behind `layout.unimplemented`. Remove the override once the selected Servo
revision depends on a Stylo that parses the property for Servo.

The feature is `servo-embed` (optional; off by default).

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
rev = "f0bb1aaf981b65c8dddc848bb647045d4c777d11"
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

- `libglvnd-devel`
- `libopenssl-devel`
- `dbus-1-devel`
- `libudev-devel`
- `libxkbcommon-devel`
- `libwayland-devel`

Install with: `sudo zypper install -y libglvnd-devel libopenssl-devel dbus-1-devel libudev-devel libxkbcommon-devel libwayland-devel`

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

Status is stated against executed evidence; code that exists but has not been
exercised is listed as unverified.

- **Embedder API**: both `weft-servo-shell` and `weft-app-shell` fail to compile
  with `servo-embed` against this revision (keyboard events, mouse actions,
  rendering readback, shutdown and window-handle APIs changed). Until they
  build and present a frame, input forwarding, surface sharing and rendering
  in the shells are unverified.
- **`backdrop-filter`**: the parsing (Stylo `f1ba496`) and rendering (Servo
  `8e7dc40`, `f0bb1aa`) patches are selected by the lockfile. Rendering is
  unverified until a pixel check passes on the shells.
- **Shell surface association**: the shells pass winit's `wl_display` and
  `wl_surface` to `zweft_shell_manager_v1.create_window` through a shared
  connection; unverified at this revision.
- **WebGPU on Mesa**: not evaluated.
- **Process separation**: each app runs as separate `weft-app-shell` and
  `weft-runtime` processes supervised by `weft-appd`. Separate processes alone
  do not confine the renderer; see `docs/security.md`.
- **SpiderMonkey**: JIT-compiled JavaScript runs with the Servo process's
  permissions; WEFT relies on SpiderMonkey's own security properties for the
  JavaScript boundary. See `docs/security.md`.

## Update policy

`servo-weft` in `marcoallegretti/servo` and `servo-weft` in
`marcoallegretti/stylo` carry WEFT-specific patches. WEFT pins exact revisions
rather than tracking a branch, so a rebuild selects the same code. A pinned
revision must stay reachable from a published branch: never force-push,
rebase or delete a branch that contains a revision a WEFT lockfile selects.
Today Servo `f0bb1aa` is reachable only from `fix/backdrop-filter-build-errors`
and Stylo `f1ba496` only from Stylo's `servo-weft`.

To move to new revisions:

1. Land the Servo or Stylo change on the fork and note its commit.
2. Update the `rev` values in `crates/weft-servo-shell/Cargo.toml`,
   `crates/weft-app-shell/Cargo.toml` and, for Stylo, the root `[patch]`.
3. Run `cargo update -p servo` (and `cargo update -p stylo` for a Stylo-only change).
4. Confirm with `grep -A2 'name = "servo"' Cargo.lock` and
   `grep -A2 'name = "stylo"' Cargo.lock` that the lockfile selects them, that
   `cargo metadata --locked` succeeds, and that `grep -c servo/stylo Cargo.lock`
   prints `0`, which shows the override applies to every Stylo crate.
5. Recompute the `outputHashes` in `infra/nixos/weft-packages.nix` for each
   changed git source.
6. Run `cargo check -p weft-servo-shell --features servo-embed --locked` and
   `cargo check -p weft-app-shell --features servo-embed --locked`, plus the
   rendering checks for any behavior the change claims, and update the tables
   in this file.

Each fork patch keeps its purpose, upstream status and removal condition here.

| Patch | Fork commit | Purpose | Upstream status | Remove when |
|-------|-------------|---------|-----------------|-------------|
| Stylo: parse `backdrop-filter` for Servo | stylo `f1ba496` | Remove the `layout.unimplemented` gate | Not submitted | Upstream Stylo parses it for Servo |
| Servo: render `backdrop-filter` | servo `8e7dc40` | Stacking context and display list support | servo/servo issue [#41567](https://github.com/servo/servo/issues/41567) | Equivalent upstream support |
| Servo: fix `build_backdrop_filter` types | servo `f0bb1aa` | Make `servo-layout` compile | Part of the patch above | Same as above |
