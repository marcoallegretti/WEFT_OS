# WEFT Application Package Format

**Status:** Design — not yet implemented.

---

## Purpose

This document specifies the on-disk format for WEFT application packages (`.wapp` files).
A `.wapp` file is the unit of distribution and installation for WEFT OS applications.

`weft-appd` uses this format to resolve an `app_id` to a launchable Wasm module and
associated UI assets.

---

## Package Identity

Each package is identified by a **reverse-domain app ID** — a dot-separated string using
DNS naming conventions:

```
com.example.notes
org.weft.calculator
io.github.username.app
```

The app ID uniquely identifies the application within a WEFT session. It is used:
- As the `app_id` field in IPC `LaunchApp` / `TerminateApp` messages.
- As the directory name under the package store root.
- In session registry lookups.

The app ID must match `^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*){2,}$`.

---

## Package Store

Packages are installed to:

```
$WEFT_APP_STORE / <app_id> /
```

Where `WEFT_APP_STORE` defaults to `/usr/share/weft/apps` (system) or
`$HOME/.local/share/weft/apps` (user). `weft-appd` searches user store first, then system
store.

The resolved package directory must contain a `wapp.toml` manifest.

---

## Directory Structure

```
<app_id>/
├── wapp.toml          # Required. Package manifest.
├── app.wasm           # Required. The WebAssembly module (Wasm 2.0).
└── ui/                # Required. UI assets served to Servo.
    ├── index.html     # Required. Entry point loaded by servo-shell.
    └── ...            # Optional CSS, JS, images.
```

No other top-level files are specified. Tools must ignore unknown files.

---

## Manifest: wapp.toml

```toml
[package]
id = "com.example.notes"
name = "Notes"
version = "1.0.0"
description = "A simple note-taking application."
author = "Example Author"

[runtime]
module = "app.wasm"

[ui]
entry = "ui/index.html"
```

### [package] fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Reverse-domain app ID. Must match identity rules above. |
| `name` | string | yes | Human-readable display name. Max 64 characters. |
| `version` | string | yes | SemVer string. |
| `description` | string | no | One-line description. Max 256 characters. |
| `author` | string | no | Author name or email. |

### [runtime] fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `module` | string | yes | Path to the Wasm module, relative to package directory. |

### [ui] fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `entry` | string | yes | Path to the HTML entry point, relative to package directory. |

---

## Wasm Module Contract

The Wasm module is run by `weft-runtime` (a separate binary embedding Wasmtime).

### Startup signal

The module signals readiness by writing the literal string `READY\n` to its standard output.
`weft-appd` monitors the process stdout; on receiving `READY\n` it transitions the session
state from `Starting` to `Running` and broadcasts `APP_READY` to connected clients.

If the module exits before writing `READY\n`, the session transitions to `Stopped` and no
`APP_READY` is sent.

If the module does not write `READY\n` within 30 seconds, `weft-appd` sends `SIGTERM` and
transitions the session to `Stopped`.

### Exit codes

| Exit code | Meaning |
|-----------|---------|
| 0 | Clean shutdown. Session transitions to `Stopped`. |
| Non-zero | Abnormal exit. Session transitions to `Stopped`. `weft-appd` logs the code. |

No automatic restart is performed by `weft-appd`. Restart policy (if any) is the
responsibility of the calling client (servo-shell).

### Stdio

- **stdin**: closed.
- **stdout**: monitored for `READY\n`. All other output is discarded.
- **stderr**: captured and forwarded to `weft-appd`'s tracing log at `WARN` level.

---

## Capability Model

Capabilities are not yet specified. This section is reserved.

Initial implementation: no capabilities are declared or checked. The Wasm module runs with
the WASI permissions granted by `weft-runtime`'s command-line configuration.

---

## Launch Flow

```
servo-shell                weft-appd               weft-runtime (child)
    |                          |                          |
    |-- LAUNCH_APP(app_id) --> |                          |
    |                          | resolve package store    |
    |                          | create session           |
    |                          | spawn weft-runtime --app |
    |                          |                       -->|
    |<-- LAUNCH_ACK(id) ---    |                          |
    |                          |        (monitors stdout) |
    |                          |<-- "READY\n" ------------|
    |                          | broadcast APP_READY      |
    |<== WS push APP_READY === |                          |
```

---

## What This Format Does Not Cover

- Package signing or verification. Not designed.
- Dependency resolution. Not designed.
- Update channels. Not designed.
- Sandboxing beyond WASI. Not designed.
- Multi-arch fat packages. Not designed. Packages are x86-64 Wasm only.
