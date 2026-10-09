---
name: weft-appd-sessions
description: Use for weft-appd session lifecycle, readiness, child supervision and cleanup, the appd IPC and WebSocket bridge, and compositor association.
---

# Appd sessions and IPC

Owner: `weft-appd` owns resolved package selection, effective grants, session
identity, child supervision, readiness and cleanup. The shells display that
state; they do not keep a second authoritative copy. Read the blueprint's
[resolved package descriptor](../../../BLUEPRINT.md#41-one-resolved-package-descriptor),
[role-separated protocol](../../../BLUEPRINT.md#61-role-separated-protocol) and
[single lifecycle owner](../../../BLUEPRINT.md#62-single-lifecycle-owner).

## Where things live

| Concern | Source |
|---|---|
| Daemon, session registry, request dispatch | [appd main.rs](../../../crates/weft-appd/src/main.rs) |
| Child spawning, preopens, systemd wrapping, data paths | [runtime.rs](../../../crates/weft-appd/src/runtime.rs) |
| Unix socket messages | [ipc.rs](../../../crates/weft-appd/src/ipc.rs) |
| Browser-facing WebSocket server | [ws.rs](../../../crates/weft-appd/src/ws.rs) |
| Compositor association client | [compositor_client.rs](../../../crates/weft-appd/src/compositor_client.rs), [weft-ipc-types](../../../crates/weft-ipc-types/src/lib.rs) |
| Image mount path | [mount.rs](../../../crates/weft-appd/src/mount.rs) |
| Injected app bridge | [app-shell embedder](../../../crates/weft-app-shell/src/embedder.rs) |
| Shell-side client | [system-ui.html](../../../infra/shell/system-ui.html) |

## Working rules

- One session model: `Starting -> Running -> Stopping -> Stopped`, with
  structured terminal reasons. `Running` requires the component initialized,
  session IPC connected, the document and bridge initialized and the surface's
  first frame presented. Stdout text cannot establish readiness.
- Every exit path enters the same cleanup: spawn failure, invalid package,
  initialization failure, timeout, cancellation, guest trap, child crash,
  explicit close and daemon shutdown. Owned resources include both children,
  portals, relays, sockets, compositor associations, temporary directories and
  mounts.
- Use a process-group or systemd mechanism that is valid for the selected
  systemd and preserves ownership; verify `systemd-run` argument combinations
  against the installed version. Terminate gracefully for a bounded time, then
  forcibly, and await settlement instead of sleeping.
- A caller-supplied `session_id` is a routing claim. Bind role, session and
  generation on the server, authenticate the browser bootstrap, and never
  deliver private payloads to unrelated subscribers.
- The bridge and daemon share one versioned semantic envelope. Bound frame size,
  queued messages and bytes and concurrency; preserve partial frames across
  cancellation; do not hold the registry lock across awaits; remove closed
  relays; resynchronize from a snapshot after overflow.
- Default logs exclude application payloads, clipboard contents, saved notes and
  credentials.

## Evidence

`python .agents/scripts/verify.py portable` runs the appd and IPC unit tests.
Lifecycle and protocol claims need fixtures with real child processes and
sockets: each startup phase failing or cancelled, children crashing, fragmented
and concurrent frames, full queues, forged and stale credentials, and checks that
no owned process, socket, relay or mount remains.
