# WEFT OS

WEFT OS is an operating system project that treats **authority boundaries** as the primary design problem.

At a high level, the system is organized around three roles:

- a **Wayland compositor** (Rust + Smithay) that owns surfaces, stacking, and input routing
- a **system shell renderer** that aims to render the desktop UI from HTML/CSS via Servo (as a Wayland client)
- an **application supervisor** that owns app lifecycle and brokers app sessions

The thesis is not “the desktop is a web page.”

The thesis is:

- a system UI can be rendered using web technologies
- without collapsing the OS into a single privileged UI process
- if the compositor and runtime boundaries remain explicit and enforced

This repository is intentionally rigorous about distinguishing:

- what is **implemented today**
- what is the **design direction**

That distinction is critical for meaningful community contribution.

## Current status (verifiable from source)

This repo is a Rust workspace (edition **2024**) with `unsafe_code` **forbidden** at the workspace lint level.

It contains working scaffolding and partial implementations for:

- `weft-compositor`
  - starts with a **winit** backend when `--winit` is passed or when a display environment is detected
  - includes a Linux-only path for a **DRM/KMS** backend
  - integrates a custom Wayland protocol XML under `protocol/` and generates server bindings
- `weft-servo-shell`
  - connects to a Wayland compositor and locates `system-ui.html`
  - **Servo embedding is not yet implemented** (the entry point currently errors intentionally)
- `weft-appd`
  - listens on a Unix socket for IPC
  - exposes a local WebSocket endpoint (127.0.0.1) for clients
  - maintains a simple app session registry and supervises a runtime process
- `weft-runtime`
  - resolves an app “package” directory and expects an `app.wasm` file
  - prints `READY` and exits cleanly as a placeholder
  - **Wasmtime integration is not yet implemented**

Public engineering documentation lives under `docs/architecture/`.

## What WEFT OS is trying to build (in-depth architecture)

WEFT OS is best understood as a set of promises. Those promises are implemented as explicit boundaries.

### 1) The compositor is the authority

In WEFT, the compositor is not “the thing that draws windows.”

It is the component that answers (consistently) the questions that become security and reliability properties:

- which surfaces exist
- which surfaces are visible, and where
- which surface receives input
- what happens when a client misbehaves or dies

Wayland’s model makes this boundary practical: clients do not get global visibility by default.

Smithay is used to build this compositor because it provides structured protocol handling and a place for centralized state, which makes it easier to express policy explicitly.

### 2) The shell is a document, but not a sovereign

WEFT’s system shell is intended to be an HTML/CSS UI rendered via Servo, running as a Wayland client.

This choice is not about using “web UI” for novelty.

It is about:

- treating layout and animation as a disciplined constraint system
- leveraging a widely understood UI language
- while keeping system authority out of the UI runtime

The shell must be able to crash without automatically taking the whole session with it.

That requires the compositor to remain authoritative.

### 3) Apps are supervised sessions, not ambient processes

WEFT treats application launch as the moment where the system can be honest about authority.

The intended model is:

- the OS creates an app session
- the OS launches the app’s runtime participant
- the OS launches the app’s UI participant
- the OS brokers their relationship

In this repo today, `weft-appd` provides the session registry and launch/supervision skeleton.

### 4) WebAssembly is an OS runtime model (not a browser feature)

WebAssembly is used in WEFT as a way to make “no ambient authority” plausible.

The core idea is capability-shaped access:

- a path string is not permission
- holding a handle is permission

This repo does not yet integrate Wasmtime, but it includes a runtime placeholder (`weft-runtime`) designed to evolve into a host-controlled Wasmtime embedder.

### 5) The core/UI channel is brokered (and must stay that way)

WEFT’s app model assumes the app has two isolated parts:

- a Wasm core
- an HTML UI

Those two still need to exchange events and state.

The direction documented in `docs/architecture/wasm-servo-channel.md` is:

- a brokered message channel owned by `weft-appd`
- structured, validated messages
- explicit backpressure and bounded payloads
- session teardown on process death

The channel is explicitly not treated as a capability transport.

### 6) Storage and packages are integrity and authority problems

WEFT’s storage stance is shaped by two goals:

- the OS should be able to answer “what is running?”
- apps should not get filesystem traversal by default

The design direction emphasizes immutable payloads, separation of code and writable data, and mediated user-intent file access (portal-style) rather than ambient path access.

Not all of this is implemented in this repository today.

It is included here because it defines what “substantial contribution” means: work that preserves explicit boundaries.

## Collaboration: let’s find the hard answers together

If you want to contribute, don’t start by asking “what feature should we add.”

Start by asking:

- what promise are we trying to make?
- what would count as evidence that we can keep it?

### High-value open problems

These are the areas where contribution has the highest leverage:

- **Servo embedding for a shell-grade Wayland client**
  - input correctness, event loop integration, stable embedding surface
- **Shell/compositor boundary**
  - the smallest protocol surface that keeps authority correct
  - object lifecycle and stale identifier rejection
- **Wasm ↔ UI channel implementation**
  - brokered transport, validation, backpressure, observability
- **Wasmtime integration in `weft-runtime`**
  - host-controlled execution, resource limits, and capability-shaped IO
- **Failure and recovery behavior**
  - compositor restart behavior
  - shell restart behavior
  - app crash containment

If you disagree with the premises, that’s also useful.

The most valuable criticism is the one that points at a boundary and says:

- “this is where the promise will break”

## Runtime knobs (verifiable from source)

This repo is early, but some runtime configuration already exists in code. These values are listed here so contributors can run components consistently and so future work doesn’t accidentally break the contract.

### `weft-servo-shell`

- `WEFT_SYSTEM_UI_HTML`
  - If set, `weft-servo-shell` will use this path as the `system-ui.html` entry point.
- `WAYLAND_DISPLAY`
  - Must be set; `weft-servo-shell` uses it to connect to the running compositor.

### `weft-appd`

- `WEFT_APPD_SOCKET`
  - If set, overrides the Unix socket path used for IPC.
- `WEFT_APPD_WS_PORT`
  - If set, overrides the local WebSocket port (defaults to `7410`).
- `XDG_RUNTIME_DIR`
  - Required; used to place runtime files such as the default Unix socket location.

### `weft-runtime`

- `WEFT_APP_STORE`
  - If set, overrides the app package store root.

## Development and validation

### Validation

On Windows PowerShell:

```powershell
./infra/scripts/check.ps1
```

On Linux:

```bash
./infra/scripts/check.sh
```

These scripts run formatting, clippy, and workspace tests (with `weft-compositor` excluded on non-Linux hosts).

### Repository layout

```text
crates/      Rust workspace members
docs/        Public engineering documentation
infra/       Validation scripts and VM workflow material
protocol/    Wayland protocol XML definitions
```
