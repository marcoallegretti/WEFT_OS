# WEFT OS Architecture

## Overview

WEFT OS is a Wayland compositor and application runtime where every app is a WebAssembly component served via a Servo-rendered WebView. The system has no ambient authority: capabilities are declared in `wapp.toml`, verified at install time, and enforced at runtime.

## Components

### weft-compositor

Smithay-based Wayland compositor. Implements the `zweft-shell-unstable-v1` protocol extension, which allows shell clients (servo-shell, app-shell) to register their surfaces as typed shell slots. Supports DRM/KMS and winit (software/dev) backends.

### weft-servo-shell

System UI host. Renders one WebView pointing at `system-ui.html` using the embedded Servo engine (feature-gated behind `servo-embed`). Connects to the compositor as window type `panel`. Dispatches the `zweft_shell_manager_v1` event queue each frame. Forwards navigation gestures received from the compositor to `weft-appd` over WebSocket.

### weft-app-shell

Per-application Servo host. Spawned by `weft-appd` after the Wasm runtime signals READY. Takes `<app_id>` and `<session_id>` as arguments. Resolves `weft-app://<app_id>/ui/index.html` and injects the `weftIpc` WebSocket bridge into the page. Registers with the compositor as window type `application`. Exits when the appd session ends.

### weft-appd

Session supervisor. Listens on a Unix socket (MessagePack protocol) and a WebSocket port (JSON). For each session: spawns `weft-runtime`, waits for READY, spawns `weft-app-shell`, manages the per-session IPC relay, and supervises child processes. Handles graceful termination and cgroup resource limits (via systemd-run when available).

### weft-runtime

WASI Preview 2 + Component Model execution host (Wasmtime 30). Loads `app.wasm` from the installed package directory. Provides host imports for `weft:app/notify`, `weft:app/ipc`, `weft:app/fetch`, `weft:app/notifications`, and `weft:app/clipboard`. Preopens filesystem paths according to capabilities declared in `wapp.toml`.

### weft-pack

Package management CLI. Subcommands: `check` (validate wapp.toml + wasm module), `sign` (Ed25519 signature), `verify` (verify signature), `generate-key`, `install`, `uninstall`, `list`, `build-image` (EROFS dm-verity), `info`.

### weft-file-portal

Per-session file proxy. Runs as a separate process with a path allowlist derived from preopened directories. Accepts JSON-lines requests over a Unix socket. Blocks path traversal. Used by apps that require file access without direct WASI preopens.

### weft-mount-helper

Setuid helper binary. Calls `veritysetup` and mounts EROFS images for dm-verity-protected packages.

## Process Topology

```
systemd
├── weft-compositor (user)
├── weft-servo-shell (user, after compositor)
└── weft-appd (user, after compositor + servo-shell)
    └── per-session:
        ├── weft-runtime <app_id> <session_id> [--preopen ...] [--ipc-socket ...]
        ├── weft-app-shell <app_id> <session_id>
        └── weft-file-portal <socket> [--allow ...]
```

## IPC

| Channel | Protocol | Purpose |
|---|---|---|
| weft-appd Unix socket | MessagePack | appd ↔ other system daemons |
| weft-appd WebSocket (:7410) | JSON | system-ui.html ↔ appd (gesture events, app lifecycle) |
| per-session IPC socket | newline-delimited JSON | weft-runtime ↔ weft-app-shell (weft:app/ipc) |
| weft-file-portal socket | JSON-lines | weft-runtime ↔ file proxy |

## Capability Enforcement

Capabilities are declared in `wapp.toml` under `[package] capabilities`. `weft-pack check` validates that only known capabilities are listed. At runtime, `weft-appd` reads capabilities and maps them to WASI preopened directories and host function availability:

| Capability | Effect |
|---|---|
| `fs:rw:app-data` | Preopen `~/.local/share/weft/apps/<id>/data` as `/data` |
| `fs:read:app-data` | Same, read-only |
| `fs:rw:xdg-documents` | Preopen `~/Documents` as `/xdg/documents` |
| `net:fetch` | Enables `weft:app/fetch` host function |
| `sys:notifications` | Enables `weft:app/notifications` host function |

## Package Format

```
<app_id>/
  wapp.toml          — manifest (id, name, version, capabilities, runtime.module, ui.entry)
  app.wasm           — WASI Component Model binary
  ui/
    index.html       — entry point served by weft-app-shell
  signature.sig      — Ed25519 signature over SHA-256 of (wapp.toml + app.wasm)
```

Package store roots (in priority order):
1. `$WEFT_APP_STORE` (if set)
2. `~/.local/share/weft/apps/`
3. `/usr/share/weft/apps/`

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `WEFT_RUNTIME_BIN` | — | Path to `weft-runtime` binary |
| `WEFT_APP_SHELL_BIN` | — | Path to `weft-app-shell` binary |
| `WEFT_FILE_PORTAL_BIN` | — | Path to `weft-file-portal` binary |
| `WEFT_MOUNT_HELPER` | — | Path to `weft-mount-helper` binary |
| `WEFT_APP_STORE` | — | Override package store root |
| `WEFT_APPD_SOCKET` | — | Unix socket path for weft-appd |
| `WEFT_APPD_WS_PORT` | `7410` | WebSocket port for weft-appd |
| `WEFT_EGL_RENDERING` | — | Set to `1` to use EGL rendering in Servo shell |
| `WEFT_DISABLE_CGROUP` | — | Set to disable systemd-run cgroup wrapping |
| `WEFT_FILE_PORTAL_SOCKET` | — | Path forwarded to app runtime for file portal |
| `XDG_RUNTIME_DIR` | — | Standard XDG runtime dir (sockets written here) |
