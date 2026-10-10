# Building WEFT OS

## Prerequisites

Linux (x86_64 or aarch64). WEFT OS is built and validated on Linux only; runtime components require Linux kernel interfaces.

System packages (openSUSE):

```sh
sudo zypper install -y \
  libwayland-devel libxkbcommon-devel \
  libglvnd-devel libgbm-devel libdrm-devel \
  libinput-devel seatd-devel libudev-devel systemd-devel \
  pkg-config clang cmake python3
```

Rust toolchain: pinned in `rust-toolchain.toml`. Run `rustup show` to confirm the active toolchain matches.

## Workspace crates (non-Servo)

```sh
cargo build --workspace \
  --exclude weft-servo-shell \
  --exclude weft-app-shell
```

These crates do not require Servo and build in under two minutes.

## weft-compositor, weft-servo-shell, weft-app-shell (Linux)

```sh
cargo build -p weft-compositor
cargo build -p weft-servo-shell
cargo build -p weft-app-shell
```

Without `--features servo-embed`, the servo-shell and app-shell binaries compile for type and lint checks but exit with an error at startup; they never report readiness. This is the default and the CI baseline.

## Servo embedding (optional, slow)

```sh
cargo build -p weft-servo-shell --features servo-embed
cargo build -p weft-app-shell --features servo-embed
```

This fetches and compiles the Servo and Stylo forks at the revisions recorded in `Cargo.lock` (see `crates/weft-servo-shell/SERVO_PIN.md`). Expect 30–60 minutes on a clean build. Servo's dependencies include SpiderMonkey (C++), which requires `clang` and `python3`.

## Frame check

`tests/frame/check_frame.py` runs `weft-compositor` nested on Xvfb with the
winit backend, starts a Servo host showing `tests/frame/reference.html`, and
checks the presented pixels. It needs `Xvfb`, `xwd`, ImageMagick's `convert`
and a Mesa OpenGL driver (llvmpipe is sufficient).

```sh
cargo build -p weft-compositor
cargo build -p weft-servo-shell -p weft-app-shell \
  --features weft-servo-shell/servo-embed,weft-app-shell/servo-embed
python3 tests/frame/check_frame.py --host system
python3 tests/frame/check_frame.py --host app
python3 tests/frame/check_frame.py --host app --slow-style 3
```

`--host app` also requires the page on screen within two seconds of
`weft-app-shell` printing `READY`; `--slow-style` delays a stylesheet that
reveals the page so premature readiness becomes visible. The stylesheet is a
file next to the page that the harness keeps from being opened with a file
lease, so the delay must stay below `/proc/sys/fs/lease-break-time`.

Logs and the last capture are written to `target/frame-check`.

`tests/frame/check_input.py` uses the same nested desktop to send real X
pointer and keyboard input (with `xdotool`) through `weft-compositor` to
`weft-servo-shell` showing `tests/frame/input.html`, and checks that the
page reacts to clicks on either side of a boundary and to the `a` and
`Shift+a` keys. Logs and the last capture are written to `target/input-check`.

```sh
python3 tests/frame/check_input.py
```

The application checks run `weft-appd` with `weft-runtime` (built with
`wasmtime-runtime`) and `weft-app-shell` (built with `servo-embed`) on the
same nested desktop and drive the demo apps with real keyboard input:

```sh
python3 tests/frame/check_counter.py   # Counter round trip through the session bridge
python3 tests/frame/check_notes.py     # Notes stores exactly what is typed, refuses stale saves, asks before closing unsaved text
python3 tests/frame/check_app_data.py  # Notes data from the earlier layout is moved and kept
python3 tests/frame/check_confinement.py  # an app page reaches only its UI files and its session bridge
python3 tests/frame/check_activation.py   # apps fill the work area above the taskbar; launch and ACTIVATE_APP raise and focus
python3 tests/frame/check_system_ui.py    # the system UI's launcher and taskbar work from the keyboard alone, reached with a Super tap
python3 tests/frame/check_update.py    # updates leave running sessions on their revision; failed updates, rollback and uninstall
```

## Demo apps (wasm32-wasip2)

Each demo is a standalone crate in `examples/`. Pre-built `app.wasm` binaries are committed; they are reproducible with the toolchain pinned in `rust-toolchain.toml`. To rebuild:

```sh
rustup target add wasm32-wasip2
cd examples/org.weft.demo.counter
cargo build --release
# output: target/wasm32-wasip2/release/app.wasm
cp target/wasm32-wasip2/release/app.wasm app.wasm
rm -rf target
```

The signature covers every file in the package directory, so remove the crate's `target` directory before signing it again. The demo packages are signed with the test key in `examples/keys`, which is a fixture, not a signing authority:

```sh
weft-pack sign examples/org.weft.demo.notes --key examples/keys/weft-sign.key
```

## weft-runtime with Wasmtime

```sh
cargo build -p weft-runtime --features wasmtime-runtime,net-fetch
```

Without `--features wasmtime-runtime`, the runtime compiles for type, lint and unit checks (the portable CI configuration) but exits with an error when asked to run a component; it never reports readiness.

## Signing packages

```sh
weft-pack generate-key ./keys
weft-pack sign ./examples/org.weft.demo.counter --key ./keys/weft-sign.key
weft-pack verify ./examples/org.weft.demo.counter --key ./keys/weft-sign.pub
```

Installation requires a signature by a trusted key. To install the demos with the demo key, trust it for your user (the key is a test fixture; do not trust it on a system you depend on):

```sh
mkdir -p ~/.config/weft/trusted-keys
cp examples/keys/weft-sign.pub ~/.config/weft/trusted-keys/weft-demo.pub
weft-pack install examples/org.weft.demo.notes
```

Unsigned packages under development install with `weft-pack install <dir> --dev`, recorded as development content and marked `(development)` by `weft-pack list`. See `docs/security.md` for the trust store and app ID ownership.

## NixOS VM (requires Nix with flakes)

Build the VM image:

```sh
bash infra/vm/build.sh
```

Run with QEMU:

```sh
bash infra/vm/run.sh
```

See `infra/nixos/weft-packages.nix` for the package derivations. Its `outputHashes` hold one hash per git source in `Cargo.lock` (the Servo and Stylo forks); every package vendors them, so they must match the pinned revisions for any Nix package to build.

Logging in on tty1 starts the user target `weft-session.target`, which binds `graphical-session.target` (that one refuses a manual start) and wants `weft-compositor`, then `weft-servo-shell` with the system UI page its package installs, and `weft-appd` with the runtime, app shell, file portal and mount helper packages. Run as a systemd user service, the compositor publishes its socket as `WAYLAND_DISPLAY` to the user manager before reporting readiness, so the units after it, and the app shells weft-appd starts, connect to it. The units in `infra/systemd/` are the same session for an installation outside Nix, as user units under `weft-session.target`. The image is not yet bootable to a working desktop:

- the Servo source hash in `outputHashes` is still a placeholder, so no package builds until a Nix build reports it;
- the shell packages do not enable `servo-embed`, so `weft-servo-shell` and `weft-app-shell` exit at startup.

The VM opens a graphics window; `tty0` is its primary console and the serial console a secondary one. `weft-mount-helper` is installed setuid root through `security.wrappers` at `/run/wrappers/bin/weft-mount-helper`, built to run cryptsetup's `veritysetup`.

## CI

Jobs run on every push to `main` and on pull requests:

- `portable` — fmt, clippy, tests for the crates that need no Linux system libraries (excludes Wayland crates)
- `linux-only` — clippy and tests for `weft-compositor`, `weft-servo-shell`, `weft-app-shell`
- `runtime-grants` — clippy and a build of `weft-runtime` with `wasmtime-runtime,net-fetch`, then `tests/runtime/check_grants.py`, which runs a probe component under each kind of capability grant
- `servo-embed-linux` — `cargo check --features servo-embed` for servo-shell and app-shell
- `contributor-toolkit` — `python .agents/scripts/verify.py toolkit`
