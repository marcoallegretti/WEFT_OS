# WEFT OS Architecture

## Overview

WEFT OS is a Wayland compositor and application runtime where every app is a WebAssembly component served via a Servo-rendered WebView. The system has no ambient authority: capabilities are declared in `wapp.toml`, verified at install time, and enforced at runtime.

## Components

### weft-compositor

Smithay-based Wayland compositor. Implements the `zweft-shell-unstable-v1` protocol extension, which allows shell clients (servo-shell, app-shell) to register their surfaces as typed shell slots. Supports DRM/KMS and winit (software/dev) backends.

### weft-servo-shell

System UI host. Renders one WebView pointing at `system-ui.html` using the embedded Servo engine (feature-gated behind `servo-embed`). Connects to the compositor as window type `panel`. Dispatches the `zweft_shell_manager_v1` event queue each frame. Forwards navigation gestures received from the compositor to `weft-appd` over WebSocket. Reads the appd WebSocket port and system token from `$XDG_RUNTIME_DIR/weft/` and hands both to the page through `window.weftAppdEndpoint(port, token)`; the shell hands the token only to the system UI document and denies top-level navigation away from it, and the launcher shows package names as text, never markup.

### weft-app-shell

Per-application Servo host. Spawned by `weft-appd` after the Wasm runtime signals READY. Takes `<app_id>`, `<session_id>` and `--ui <path>`, the UI document weft-appd resolved from the package; it never looks for the package itself. Injects the `weftIpc` WebSocket bridge into the page. The bridge authenticates with the per-session token from `WEFT_BRIDGE_TOKEN` and can only exchange messages with its own session. It is installed only in top-level documents inside the application's UI directory, and top-level navigation outside that directory is denied; framed documents never receive the bridge. Every resource the page loads (subresources, `fetch()`, XHR, workers and WebSocket handshakes) is checked by the shell: the page may load files inside its UI directory, also after symbolic links are resolved, inline `data:` and `blob:` resources, `about:blank` and the session bridge; any other local file or network destination, and every redirect, fails with a network error. Loads Servo does not attribute to the webview, such as `navigator.sendBeacon()`, are all refused. Network access belongs to the Wasm component, under its fetch grants. Servo's privileged `navigator.servo` interface, which can change engine preferences, is not exposed to `about:blank` or `about:srcdoc` documents (a fork patch; see `crates/weft-servo-shell/SERVO_PIN.md`). Registers with the compositor as window type `application`. Exits when the appd session ends.

### weft-appd

Session supervisor. Listens on a Unix socket (MessagePack protocol) and a WebSocket port (JSON). Before a session exists, appd resolves the package once (see [Package resolution](#package-resolution)) and derives its grants from that package's manifest; the runtime and the app shell receive the resolved component and UI document and do not search the stores themselves. For each session: spawns `weft-runtime` and waits for its READY, spawns `weft-app-shell` (`WEFT_APP_SHELL_BIN` is required) and waits for its READY, and only then reports the session running (`APP_READY`). A child reports readiness by printing `READY <token>` on stdout, where the token is a random per-session value appd passes in `WEFT_READY_TOKEN`; Wasm guests and pages share those stdout streams but cannot read the token, so their output cannot fake readiness. Each wait times out after 30 seconds. If either child fails to start, fails to become ready or exits, or the session is terminated, the whole session is stopped: both children, the IPC relay (its task, socket and registered sender), the file portal, the compositor association and any image mount are released. The IPC relay is created with the session in `$XDG_RUNTIME_DIR/weft` (a session cannot start without it); its socket is accessible only to the user and accepts a single connection, which the session's runtime makes before it starts the component. If that connection closes while the session runs, the session is stopped. `IPC_FORWARD` on the Unix socket never waits: it reports an error when the session has no relay or its queue is full. appd applies cgroup resource limits to the runtime via systemd-run when available.

### weft-runtime

WASI Preview 2 + Component Model execution host (Wasmtime 30). Runs the component weft-appd passes with `--module <path>`, the package's `[runtime].module`. Provides host imports for `weft:app/notify`, `weft:app/ipc`, `weft:app/fetch`, `weft:app/notifications`, and `weft:app/clipboard`. Preopens filesystem paths according to capabilities declared in `wapp.toml`.

### weft-pack

Package management CLI. Subcommands: `check` (validate wapp.toml, entries and package content), `sign` (Ed25519 signature over every file), `verify` (verify signature), `generate-key`, `install` (requires a signature by a trusted key unless `--dev`; records the app ID's owner; installs or updates, see [Installing and updating](#installing-and-updating)), `uninstall`, `rollback`, `list`, `build-image` (EROFS dm-verity), `info`.

### weft-file-portal

Per-session file proxy. Runs as a separate process with a path allowlist derived from the session's directory grants; read-only grants (`--allow-read`) refuse writes. Paths are checked lexically, so `..` is blocked but symbolic links are followed. Accepts JSON-lines requests over a Unix socket. Components cannot currently reach that socket, because the runtime grants them no socket access.

### weft-mount-helper

Setuid helper binary. Calls `veritysetup` and mounts EROFS images for dm-verity-protected packages. Its device-mapper device is named after the mountpoint, which weft-appd names with a random token in `$XDG_RUNTIME_DIR/weft/mnt`. The helper does not yet restrict its callers or the images and mountpoints they pass, so it is not part of a supported installation.

## Process Topology

```
systemd
├── weft-compositor (user)
├── weft-servo-shell (user, after compositor)
└── weft-appd (user, after compositor + servo-shell)
    └── per-session:
        ├── weft-runtime <app_id> <session_id> --ipc-socket <path> --module <path> [--preopen HOST::GUEST::ro|rw]... [--grant CAP]...
        ├── weft-app-shell <app_id> <session_id> --ui <path>
        └── weft-file-portal <socket> [--allow ...] [--allow-read ...]
```

## IPC

| Channel | Protocol | Purpose |
|---|---|---|
| weft-appd Unix socket | MessagePack | appd ↔ other system daemons |
| weft-appd WebSocket (:7410) | JSON | system-ui.html and servo-shell ↔ appd (system role: gestures, app lifecycle); app pages ↔ their session (app role) |
| per-session IPC socket | newline-delimited JSON | weft-runtime ↔ weft-app-shell (weft:app/ipc) |
| weft-file-portal socket | JSON-lines | weft-runtime ↔ file proxy |

### WebSocket roles

Every WebSocket connection must complete the upgrade and send `HELLO` as its first message within 5 seconds, or appd closes it; a first message that is not a valid `HELLO` is answered with an error (code 401) before the connection is closed. Messages are limited to 1 MiB.

- `{"type":"HELLO","role":"system","token":"<hex>"}` — the token is the content of `$XDG_RUNTIME_DIR/weft/appd.systoken` (mode 0600), regenerated each time appd starts. A system client may launch, terminate and query sessions. It receives lifecycle broadcasts but never application messages, and cannot inject them (`IPC_FORWARD` is refused with code 403).
- `{"type":"HELLO","role":"app","session_id":<n>,"token":"<hex>"}` — the token is the session's bridge token, passed to `weft-app-shell` in `WEFT_BRIDGE_TOKEN` and distinct from its readiness token. An app client may only send `{"type":"APP_MESSAGE","payload":"<string>"}` (at most 64 KiB, no newlines); the payload goes to its own session's Wasm component. It receives only its own session's replies, as `APP_MESSAGE`, and is disconnected when the session stops.

## Capability Enforcement

Capabilities are declared in `wapp.toml` under `[package] capabilities`. One vocabulary (`weft_ipc_types::capability`) is used by `weft-pack check`, which rejects unknown strings, by `weft-appd`, which derives the session's effective grants, and by `weft-runtime`, which enforces them.

`weft-appd` derives grants once, before a session exists. `LAUNCH_APP` is answered with an error and nothing starts when the app ID is malformed (code 400), the package is not installed (404), a declared capability is unknown, unsupported on this host or cannot be satisfied (403), app data exists in both the current and the earlier location (409), or the host cannot provide the resources (500, for example without `HOME`). All capabilities are checked before the app's data directory is created. Without `WEFT_RUNTIME_BIN`, appd starts no processes and derives no grants. The runtime does not read the manifest; it receives the grants as arguments:

- `--preopen HOST::GUEST::ro|rw` for each directory, with the access mode the capability names;
- `--grant <capability>` for each host-import capability.

Each capability-controlled host import (fetch, notifications, clipboard) checks its grant on every call and returns an error naming the missing capability when it is not granted. `notify` and `ipc` need no capability.

| Capability | Effect |
|---|---|
| `fs:read:app-data` / `fs:rw:app-data` | Preopen the app's data directory (see *App data*) as `/data`, read-only or read-write |
| `fs:read:xdg-documents` / `fs:rw:xdg-documents` | Preopen the documents directory from the XDG user-dirs configuration (`XDG_DOCUMENTS_DIR`) as `/xdg/documents`; the launch fails when none is configured |
| `net:fetch:<host>` | `weft:app/fetch` to that exact host (a lowercase DNS name or dotted IPv4 address) over HTTP or HTTPS, on any port |
| `net:fetch:*` | `weft:app/fetch` to any host |
| `sys:notifications` | `weft:app/notifications` |
| `sys:clipboard:read` / `sys:clipboard:write` | `weft:app/clipboard#read` / `#write` |
| `hw:gpu:compute` / `hw:gpu:render` | Not supported; the launch fails |

Fetch does not follow redirects: a redirect is returned to the component as a response, so every destination it requests is checked against its grants. HTTP error statuses are responses too; only policy and transport failures are errors. Only the methods GET, HEAD, POST, PUT, PATCH, DELETE and OPTIONS are accepted, and a component cannot set `Host`, `Content-Length`, `Transfer-Encoding` or other hop-by-hop headers. Connecting times out after 10 seconds and the whole request after 30 seconds, not counting DNS resolution; response bodies are limited to 16 MiB. Fetch requires a runtime built with the `net-fetch` feature.

Fetch grants name hosts, not addresses: ports are not restricted, and a host that resolves to a loopback or private address (or `net:fetch:*`) can reach local services. IPv6 literals cannot be granted.

`tests/runtime/check_grants.py` runs the real runtime on a probe component (`tests/components/grants-probe`) under each kind of grant.

## Package Format

```
<app_id>/
  wapp.toml          — manifest (id, name, version, capabilities, runtime.module, ui.entry)
  app.wasm           — WASI Component Model binary
  ui/
    index.html       — entry point served by weft-app-shell
  signature.sig      — Ed25519 signature over the content digest of every other file (see security.md)
```

Package store roots (in priority order):
1. `$WEFT_APP_STORE` (if set)
2. `~/.local/share/weft/apps/`
3. `/usr/share/weft/apps/`

### Package resolution

`weft-appd` resolves a launch once. A verified image in any store root takes precedence over a directory install in any root, so a user's directory cannot shadow a system image; otherwise the first root with a directory install is used:

- **Verified image:** `<root>/<id>.app.img` with its dm-verity hash tree `<id>.app.hash` and root hash `<id>.app.roothash`, the names `weft-pack build-image` and `build-verity` produce. When any of the three exists, the image must mount through `weft-mount-helper`; a missing companion, a malformed root hash, a missing helper or a failed mount refuses the launch, and appd never falls back to a directory install. The root hash is read from the store next to the image and is not yet authenticated, so dm-verity detects corruption but not a store whose image, hash tree and root hash were replaced together. The mount belongs to the session and is released when the session stops.
- **Directory install:** `<root>/<id>`, a link to the active revision `.revisions/<id>/<digest>` written by `weft-pack`, or, for a store written before revisions, a package directory. A link is followed only in exactly that form, to one of the app's own revisions; any other link is refused (403). The session runs from the revision directory itself and holds a shared lock on it until it stops, so it keeps its bytes while an update or rollback switches the link.

The package root must be a directory and its manifest a regular file, neither a symbolic link, and the manifest must declare the requested ID. `[runtime].module` and `[ui].entry` must be relative paths of plain components, without symbolic links, naming regular files inside that root. The component, the UI document and the capabilities all come from that one manifest. A refused launch reports 400 (invalid ID), 404 (not installed), 403 (invalid package, image or image metadata) or 500 (the host cannot read the package or mount the image).

A directory install can still be changed by its owner while a session runs; for a verified owner, appd checks its content against the signature at launch (see security.md).

### Installing and updating

`weft-pack install` copies the package into a staging directory in the store and checks, verifies and admits that copy, so the bytes that are checked are the bytes that are installed. The copy is flushed to disk and renamed to an immutable revision named by its content digest, `<root>/.revisions/<id>/<digest>`; installing the same content again reuses it. One rename of the link `<root>/<id>` then makes it active. An installation that fails, or is interrupted at any point, leaves the previously active revision active, or, for a first installation, nothing installed and no owner record created by it. A package directory from before revisions is replaced in one exchange with the new link, only while no session runs from it.

The revision active before an update is kept for `weft-pack rollback <id>`, which makes it active again, keeping the revision it replaces in turn; it is checked against the owner record again, so a revision whose publisher key was withdrawn is not rolled back to. Other revisions are removed by each install, rollback or uninstall of the app, except those a session still runs from; those stay until a later operation on the app finds them unused. Temporary links of interrupted activations are removed then too, and staging, unpacking and removal directories once they are an hour old. `weft-pack uninstall` removes the link, so new launches no longer find the app, and the kept revision; running sessions continue on their revisions. App data is never touched by any of these operations. Package revisions and app data are independent: rolling back a package does not roll back its data.

## App data

Each app's private data lives in `$XDG_DATA_HOME/weft/app-data/<id>` (`~/.local/share/weft/app-data/<id>` by default), apart from installed packages. Installing, updating or uninstalling a package never removes it. `weft-appd` creates the directory when a session with an app-data capability starts and makes it accessible only to the user (mode 0700); a symbolic link in its place is refused rather than followed. `weft-appd` and `weft-pack` must run with the same `HOME` and `XDG_DATA_HOME` to agree on this location; the systemd unit allows `weft-appd` to write under `~/.local/share/weft`.

Earlier versions kept app data inside the user package store, in `~/.local/share/weft/apps/<id>/data`, for apps declaring an app-data capability. That directory is moved to the new location in a single rename, which never replaces an existing directory:

- by `weft-appd`, on the first launch with an app-data capability;
- by `weft-pack uninstall` and `weft-pack install`, before they touch a package directory in the user store (however its path is spelled). `uninstall` moves it when the package declares an app-data capability or its manifest cannot be read, and otherwise stops without removing anything; a `data` directory is never deleted with a package.

A package directory left empty by the move is removed. Nothing is moved, and `weft-pack` stops or the launch is refused, when the new location already exists, even empty (error 409), when the old one is not a plain directory, or when the two are on different filesystems (error 500); the message names both locations so the user can keep one copy and remove the other. The system UI shows such refusals. Packages cannot contain a top-level `data` entry, so package content is never taken for app data. `tests/frame/check_app_data.py` checks that existing Notes data arrives byte for byte, that the Notes session then reads and saves it, and that a conflict is refused.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `WEFT_RUNTIME_BIN` | — | Path to `weft-runtime` binary |
| `WEFT_APP_SHELL_BIN` | — | Path to `weft-app-shell` binary; required to launch apps |
| `WEFT_READY_TOKEN` | set by appd | Per-session readiness token passed to `weft-runtime` and `weft-app-shell` |
| `WEFT_BRIDGE_TOKEN` | set by appd | Per-session token `weft-app-shell` uses to join its session on the appd WebSocket |
| `WEFT_FILE_PORTAL_BIN` | — | Path to `weft-file-portal` binary |
| `WEFT_MOUNT_HELPER` | — | Path to `weft-mount-helper` binary |
| `WEFT_APP_STORE` | — | Override package store root |
| `WEFT_APPD_SOCKET` | — | Unix socket path for weft-appd |
| `WEFT_APPD_WS_PORT` | `7410` | WebSocket port for weft-appd |
| `WEFT_EGL_RENDERING` | — | Set to `1` to use EGL rendering in Servo shell |
| `WEFT_DISABLE_CGROUP` | — | Set to disable systemd-run cgroup wrapping |
| `WEFT_FILE_PORTAL_SOCKET` | — | Path forwarded to app runtime for file portal |
| `XDG_RUNTIME_DIR` | — | Standard XDG runtime dir (sockets written here) |
