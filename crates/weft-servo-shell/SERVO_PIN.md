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

Each frame is painted with `WebView::paint` and then presented:

- **Software (default):** Servo renders into a `SoftwareRenderingContext`.
  The host reads the painted frame back with `read_to_image`, presents the
  context and copies the pixels into a persistent `softbuffer` surface on the
  winit window.
- **EGL:** with `WEFT_EGL_RENDERING=1` the host creates a
  `WindowRenderingContext` from winit's display and window handles and
  presents it directly. If that fails it falls back to software.

Neither host presents anything before Servo's first repaint request; the
first frames can still be blank or unstyled while the document loads. Both
enable Servo's `layout_grid_enabled` preference, because the system UI and
application pages use CSS Grid, which Servo disables by default.

`weft-app-shell` prints `READY` after it presents a frame once the document
has loaded and Servo reports its rendering settled: on load completion it
requests `WebView::take_screenshot`, whose callback runs only after `load` has
fired in every frame, render-blocking resources, images and web fonts are done
and the rendering is up to date. `weft-appd` does not read this signal yet; it
treats the runtime's `READY` as session readiness. Neither host prints `READY`
when built without `servo-embed`; both exit with an error instead.

`tests/frame/check_frame.py` checks this on a real display path: it runs
`weft-compositor` nested on Xvfb and checks the pixels each host presents for
`tests/frame/reference.html`. For the app host it requires the page on screen
within two seconds of `READY`; `--slow-style` delays a render-blocking
stylesheet that reveals the page, which makes premature readiness visible. A
build that printed `READY` on its first presented frame failed every
`--slow-style 4` run; that build was a local experiment, not part of history.

## Known limitations at this pin

Status is stated against executed evidence; code that exists but has not been
exercised is listed as unverified.

- **Verified rendering path:** both hosts present the reference page through
  `weft-compositor`'s winit backend on Xvfb with Mesa 25.2.8 llvmpipe, using
  the software path (Ubuntu 24.04, x86_64; `check_frame.py --host system`,
  `--host app` and `--host app --slow-style 3`, five runs each). The EGL path,
  DRM sessions, resize, output scale and idle wakeup are unverified.
- **Input:** `tests/frame/check_input.py` verifies, on the same nested
  setup, that left clicks 6 px either side of a boundary reach the element
  under the pointer, and that `a` and `Shift+a` key presses reach the page with
  the right key value and modifier, using real X input that `weft-compositor`
  routes to `weft-servo-shell` (three runs). The harness gives the compositor's
  X window input focus, as a window manager would; the client's keyboard focus
  comes from the compositor's click-to-focus. Unverified: key release and
  repeat, pointer motion, enter/leave and button release events, other buttons,
  wheel, focus loss, non-US layouts, non-ASCII text, scale changes and
  `weft-app-shell` input.
- **`backdrop-filter`**: the parsing (Stylo `f1ba496`) and rendering (Servo
  `8e7dc40`, `f0bb1aa`) patches are selected by the lockfile; rendering is
  unverified.
- **Resources:** the hosts install no Servo resource reader, so Servo logs
  `Resource reader not set` and uses empty resources: the public suffix list,
  HSTS preload list, Bluetooth blocklist, certificate and network error pages,
  crash and directory-listing pages and media controls. Servo falls back to its
  built-in placeholder image.
- **Window decorations:** `weft-compositor` offers no server-side decorations,
  so winit draws a client-side title bar on both hosts' windows, including the
  system panel.
- **Shell surface association**: the shells pass winit's `wl_display` and
  `wl_surface` to `zweft_shell_manager_v1.create_window` through a shared
  connection; unverified.
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
