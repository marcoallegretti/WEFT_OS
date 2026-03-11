# SpiderMonkey Bridge Assessment

**Status:** Assessment complete — blocker 3 resolved.
**Decision:** Custom SpiderMonkey bindings are not required. WebSocket satisfies the UI endpoint contract.

---

## Context

The weft-appd IPC channel design (see `docs/architecture/wasm-servo-channel.md`) requires a
UI endpoint: a mechanism by which the system UI HTML page running inside Servo can receive
notifications from `weft-appd` (e.g., `APP_READY`) and send requests (e.g., `LAUNCH_APP`).

The original concern ("blocker 3") was that satisfying this requirement might need custom
SpiderMonkey bindings injected into Servo — functionality that is not stable or exposed as a
public Servo API.

---

## What the UI Endpoint Requires

1. The system-ui.html page must be able to **send** structured messages to `weft-appd`
   (LAUNCH_APP, TERMINATE_APP, QUERY_RUNNING, QUERY_APP_STATE).
2. The system-ui.html page must be able to **receive** asynchronous push notifications from
   `weft-appd` (LAUNCH_ACK, APP_READY).
3. Messages must carry a session identifier and structured payload.
4. The transport must work over a local connection (same machine, session-local).

None of these requirements are specific to SpiderMonkey internals. They describe a generic
bidirectional message channel.

---

## Servo's Relevant Capabilities (as of 2025)

| Feature | Status | Notes |
|---------|--------|-------|
| WebSocket API (RFC 6455) | Implemented | `servo/components/script/dom/websocket.rs` |
| fetch() | Implemented | Standard Fetch API |
| JSON.parse / JSON.stringify | Implemented | Core JS, SpiderMonkey |
| `addEventListener` / `dispatchEvent` | Implemented | Full DOM events |
| ES modules | Partially implemented | Not required for this use case |
| Service Workers | Not implemented | Not required |
| Custom native APIs via embedder | Not stable / no public API | This was the original concern |

**Key finding:** WebSocket is implemented in Servo and is the standard API for persistent
bidirectional message channels between a web page and a local service.

---

## Proposed Implementation: WebSocket Endpoint in weft-appd

`weft-appd` already has `tokio` with the `net` feature. Adding a WebSocket server alongside
the existing Unix socket server requires only the `tokio-tungstenite` crate (tokio-native
WebSocket).

### Transport architecture

```
system-ui.html (Servo)
  └─ WebSocket ws://127.0.0.1:<port>/appd
        │
        │  JSON frames (human-readable, same semantics as MessagePack IPC)
        │
weft-appd WebSocket listener
  └─ shared SessionRegistry (same Arc<Mutex<...>>)
```

The Unix socket remains the production IPC path for machine-to-machine communication
(servo-shell native path). The WebSocket listener is the UI endpoint for Servo.

### Message format

The same `Request` / `Response` enum used by the MessagePack Unix socket path is also
serialized as JSON for the WebSocket channel (using `serde_json`). The type tag remains
`SCREAMING_SNAKE_CASE`.

Example:
```json
{"type":"LAUNCH_APP","app_id":"com.example.notes","surface_id":0}
```

### Port selection

`weft-appd` binds on `127.0.0.1` only. The port is resolved in priority order:
1. `WEFT_APPD_WS_PORT` environment variable
2. Default: `7410`

The port is written to `$XDG_RUNTIME_DIR/weft/appd.wsport` at startup so that the
system-ui.html can discover it without hardcoding.

### Push notifications

The WebSocket connection stays open. `weft-appd` pushes `APP_READY` frames to all connected
clients when a session transitions to `Running`. This is implemented with a `tokio::sync::broadcast`
channel that the WebSocket handler subscribes to.

---

## What This Assessment Does Not Cover

- **Servo's Wayland EGL surface stability**: covered in `docs/architecture/winit-wayland-audit.md`.
- **Servo DPI / HiDPI rendering**: not assessed; deferred.
- **Content Security Policy interactions with ws://127.0.0.1**: `system-ui.html` is served
  from the local filesystem (file://) or a bundled origin. CSP headers are controlled by
  WEFT, so `ws://127.0.0.1` can be explicitly allowed.
- **Servo WebSocket reconnect on compositor restart**: not designed; out of scope for initial
  implementation.

---

## Blocker 3 Resolution

The original concern was that the UI endpoint would require injecting custom SpiderMonkey
bindings into Servo, which is not a stable operation.

**This concern is resolved.** The UI endpoint is implemented entirely via the standard
WebSocket API, which Servo ships as a fully functional implementation. No custom SpiderMonkey
bindings are needed. The WEFT codebase does not modify Servo's internals for this feature.

**Implementation required:**
1. Add `tokio-tungstenite` + `serde_json` to `weft-appd`.
2. Add `broadcast::Sender<Response>` to `run()` for push notifications.
3. Start a WebSocket listener task alongside the existing Unix socket listener.
4. Update `system-ui.html` to connect via WebSocket and handle `APP_READY` events.

This work is scoped and ready to implement.
