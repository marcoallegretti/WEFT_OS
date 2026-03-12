# Building WEFT OS

## Prerequisites

Linux (x86_64 or aarch64). Building on Windows is supported for workspace validation only; runtime components require Linux kernel interfaces.

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

Without `--features servo-embed`, the servo-shell and app-shell stubs compile and print READY without running an actual WebView. This is the default and the CI baseline.

## Servo embedding (optional, slow)

```sh
cargo build -p weft-servo-shell --features servo-embed
cargo build -p weft-app-shell --features servo-embed
```

This fetches and compiles the Servo fork (`github.com/marcoallegretti/servo`, branch `servo-weft`). Expect 30–60 minutes on a clean build. Servo's dependencies include SpiderMonkey (C++), which requires `clang` and `python3`.

## Demo apps (wasm32-wasip2)

Each demo is a standalone crate in `examples/`. Pre-built `app.wasm` binaries are committed. To rebuild:

```sh
rustup target add wasm32-wasip2
cd examples/org.weft.demo.counter
cargo build --release
# output: target/wasm32-wasip2/release/app.wasm
```

## weft-runtime with Wasmtime

```sh
cargo build -p weft-runtime --features wasmtime-runtime,net-fetch
```

Without `--features wasmtime-runtime`, the runtime prints READY and exits (stub mode, used in CI on platforms without Linux system dependencies).

## Signing packages

```sh
weft-pack generate-key ./keys
weft-pack sign ./examples/org.weft.demo.counter --key ./keys/weft-sign.key
weft-pack verify ./examples/org.weft.demo.counter --key ./keys/weft-sign.pub
```

## NixOS VM (requires Nix with flakes)

Build the VM image:

```sh
bash infra/vm/build.sh
```

Run with QEMU:

```sh
bash infra/vm/run.sh
```

See `infra/nixos/weft-packages.nix` for the package derivations. The `outputHashes` entry for the Servo git dependency must be filled in before the `servo-embed` packages will build under Nix.

## CI

Three jobs run on every push to `main` and on pull requests:

- `cross-platform` — fmt, clippy, tests on Ubuntu and Windows (excludes Wayland crates)
- `linux-only` — clippy and tests for `weft-compositor`, `weft-servo-shell`, `weft-app-shell`
- `servo-embed-linux` — `cargo check --features servo-embed` for servo-shell and app-shell
