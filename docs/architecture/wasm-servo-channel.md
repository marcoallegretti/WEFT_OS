# Wasm–Servo Channel Design

This document defines the initial WEFT direction for communication between an application's Wasm core and its HTML UI.

## Status

Defined as the WEFT repository direction for implementation planning.

Not implemented.

Requires further upstream-facing discussion where Servo integration points are affected.

## Problem statement

A WEFT application has two isolated parts:

- `core.wasm` running under Wasmtime
- `ui/index.html` rendered by Servo

They must exchange application events and state updates without collapsing process isolation or introducing ambient authority.

## Decision

The initial WEFT direction is a brokered message channel with these properties:

- `weft-appd` owns the session
- the Wasm core and HTML UI are peers, not parent and child
- all payloads are structured messages
- all messages cross an explicit broker boundary
- neither side receives direct ambient access to system resources through the channel

## Transport shape

The selected transport model is:

- one app session broker owned by `weft-appd`
- one message stream from Wasm core to broker
- one message stream from UI to broker
- broker validation before delivery in either direction

The transport must support:

- ordered delivery within a session
- bounded message size
- explicit backpressure
- session teardown on process death

This document does not lock the implementation to a specific byte framing library.

It does lock the architecture to brokered message passing rather than shared memory or in-process embedding.

## Why this direction was selected

This direction was selected because it preserves the design constraints already established in the authoritative blueprint:

- process isolation remains intact
- capability mediation stays outside the UI runtime
- the shell and app model do not depend on embedding Wasmtime into Servo
- the design does not depend on Worker support being complete in Servo
- the design avoids shared-memory synchronization complexity in the first implementation track

## Explicitly rejected directions for the initial WEFT track

### Shared memory ring buffer

Rejected for the initial track because it increases synchronization complexity, failure complexity, and debugging cost before the protocol contract is stable.

### In-process Wasmtime hosted directly inside Servo

Rejected for the initial track because it collapses the process boundary too early and would require a much larger Servo-side architectural commitment before WEFT has closed the surrounding interface contracts.

### Worker-based UI execution model as the primary plan

Rejected for the initial track because Worker support should not be treated as a closed dependency assumption for WEFT planning.

## Session model

Each launched app receives an app session identified by a session identifier created by `weft-appd`.

The session owns:

- the approved capability set
- the Wasm process handle
- the UI browsing-context handle
- the two channel endpoints
- the teardown state

A session ends when either side dies, the app is closed, or the broker invalidates the session.

## Message model

All messages are structured and versioned.

Every message includes:

- `session_id`
- `stream_id`
- `message_type`
- `sequence`
- `payload`

Message classes:

- UI event messages
- state update messages
- lifecycle messages
- error messages

The broker validates message class and payload shape before forwarding.

## Capability boundary

The channel is not a capability transport.

Capabilities are granted only through app launch and host-managed handles.

The UI cannot escalate privileges by sending broker messages.

The Wasm core cannot mint new authority by sending channel payloads.

## Failure model

### Wasm process crash

If the Wasm process dies:

- the broker closes the Wasm endpoint
- the UI receives a terminal session error event
- the app session is torn down unless explicit recovery is later designed

### UI crash

If the UI browsing context dies:

- the broker closes the UI endpoint
- the Wasm endpoint is terminated by `weft-appd`
- the app session ends

### Broker restart or broker failure

If the broker fails, the session is invalid.

Both sides must be treated as disconnected.

Session recovery is not implicit.

## Performance budget

The initial design target is correctness and isolation first.

Performance requirements for future implementation work:

- message transport must not permit unbounded queue growth
- large binary payloads are out of scope for the control channel
- high-frequency UI state churn must be coalesced before transport where possible

This document does not claim a measured latency target because no implementation exists yet.

## Observability requirements

Any implementation of this design must expose:

- session creation and teardown events
- message validation failures
- endpoint disconnect reasons
- queue pressure indicators

## Open implementation questions

These questions remain open for the implementation phase and any Servo-facing discussion:

- the exact UI-side binding surface presented to application JavaScript
- the exact framing and serialization format
- whether the UI endpoint is surfaced through a custom embedder API or another browser-facing transport mechanism
- whether a limited reconnect path is worth supporting

The open questions do not change the architectural decision that the channel is brokered and process-separated.
