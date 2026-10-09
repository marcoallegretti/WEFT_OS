# WEFT OS

WEFT OS is a Wayland compositor and application runtime where every app is a WebAssembly component rendered in an isolated Servo WebView. No capability is granted by default; all resource access is declared in a per-app manifest and enforced at runtime.

## What is implemented

**Compositor** — `weft-compositor` is a Smithay-based Wayland compositor with DRM/KMS and winit backends. It implements the `zweft-shell-unstable-v1` protocol extension, which typed shell slots (panel, application) register against.

**System shell** — `weft-servo-shell` embeds Servo (feature-gated, `--features servo-embed`) and renders `system-ui.html` as a Wayland panel. Without `servo-embed`, the binary exits with an error instead of rendering. Navigation gestures from the compositor are forwarded to `weft-appd` over WebSocket.

**App shell** — `weft-app-shell` is a per-process Servo host for application WebViews. It resolves `weft-app://<id>/ui/index.html`, injects a `weftIpc` WebSocket bridge into the page, and registers with the compositor as an application surface. Also feature-gated behind `servo-embed`.

**App daemon** — `weft-appd` supervises sessions: spawns `weft-runtime`, waits for its READY, spawns `weft-app-shell` and reports the session running only after the app shell's READY, manages the per-session IPC relay between the Wasm component and the WebView, and handles session teardown. Wraps processes in systemd scopes (`CPUQuota=200%`, `MemoryMax=512M`) when available.

**Runtime** — `weft-runtime` runs WASI Component Model binaries under Wasmtime 30 (`--features wasmtime-runtime`). Provides `weft:app/notify`, `weft:app/ipc`, `weft:app/fetch`, `weft:app/notifications`, and `weft:app/clipboard` host imports. Preopens filesystem paths according to declared capabilities.

**Package management** — `weft-pack` handles check, sign, verify, install, uninstall, list, build-image (EROFS dm-verity), and info. Validates capability strings at check time. App data is stored apart from packages and survives uninstall.

**File portal** — `weft-file-portal` is a per-session file proxy with a path allowlist and `..` blocking.

**Mount helper** — `weft-mount-helper` is a setuid helper for EROFS dm-verity mount/umount via `veritysetup`.

**Demo apps** — `examples/org.weft.demo.counter` and `examples/org.weft.demo.notes` are pre-built Wasm Component binaries (`wasm32-wasip2`, wit-bindgen 0.53) with HTML UIs, signed with a committed demo keypair.

## Repository layout

```
crates/           Rust workspace members
examples/         Demo app packages (wasm32-wasip2, not workspace members)
  keys/           Demo Ed25519 keypair
protocol/         zweft-shell-unstable-v1 Wayland protocol XML
infra/
  nixos/          NixOS VM configuration and package derivations
  scripts/        check.ps1, check.sh
  shell/          system-ui.html, weft-ui-kit.js
  systemd/        service unit files
  vm/             build.sh, run.sh (QEMU)
docs/
  architecture.md Component map, IPC, capability table, env vars
  security.md     Capability model, process isolation, SpiderMonkey security boundary
  building.md     Build instructions for all targets
```

## Building

Linux system packages required (openSUSE):

```sh
sudo zypper install -y \
  libwayland-devel libxkbcommon-devel libglvnd-devel \
  libgbm-devel libdrm-devel libinput-devel seatd-devel libudev-devel \
  systemd-devel pkg-config clang cmake python3
```

Build non-Servo crates:

```sh
cargo build --workspace --exclude weft-servo-shell --exclude weft-app-shell
```

Build Linux-only crates (no Servo):

```sh
cargo build -p weft-compositor -p weft-servo-shell -p weft-app-shell
```

Build with Servo embedding (30–60 min, requires clang + python3):

```sh
cargo build -p weft-servo-shell --features servo-embed
cargo build -p weft-app-shell --features servo-embed
```

See `docs/building.md` for full instructions including Wasm component builds, NixOS VM, and signing.

## CI

Jobs on every push to `main` and on pull requests:

- `portable` — fmt, clippy and tests for the crates that need no Linux system libraries
- `linux-only` — clippy and tests for compositor and shell crates
- `runtime-grants` — clippy for `weft-runtime` with its Wasm engine and fetch, and `tests/runtime/check_grants.py`
- `servo-embed-linux` — `cargo check --features servo-embed` for both servo crates
- `contributor-toolkit` — links, metadata and catalog of the contributor toolkit, and runner tests

## Security

See `docs/security.md`. Key points:

- Capabilities declared in `wapp.toml`, validated at install, enforced at runtime
- Per-app OS processes with systemd cgroup limits
- WASI filesystem isolation via preopened directories
- Ed25519 package signing; optional EROFS dm-verity
- Optional seccomp BPF blocklist in `weft-runtime`
- SpiderMonkey is not sandbox-isolated beyond process-level isolation (see `docs/security.md`)

## Servo fork

- Servo: `https://github.com/marcoallegretti/servo` at `e5d9a518`, pinned by revision
- Stylo: `https://github.com/marcoallegretti/stylo` at `f1ba4969`, through the root `[patch]`
- Patches: `backdrop-filter` parsing (Stylo) and rendering (Servo), rooting of compiled classic scripts and `navigator.servo` exposure (Servo)
- See `crates/weft-servo-shell/SERVO_PIN.md` for Servo integration status and known limitations

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). [AGENTS.md](AGENTS.md) is the engineering contract and [BLUEPRINT.md](BLUEPRINT.md) the accepted direction toward `0.1.0`.
