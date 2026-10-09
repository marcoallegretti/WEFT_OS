---
name: weft-runtime-capabilities
description: Use for the Wasmtime runtime, WIT host imports, WASI preopens, effective grants, the file portal and seccomp.
---

# Runtime, WASI and capabilities

Owner: `weft-runtime` instantiates the component and enforces every host call;
appd derives the effective grants once from the resolved package. Portals and
host services act only on explicitly granted resources. Read the blueprint's
[security and capability contract](../../../BLUEPRINT.md#5-security-and-capability-contract)
and [component compatibility](../../../BLUEPRINT.md#42-manifest-and-component-compatibility).

## Where things live

| Concern | Source |
|---|---|
| Engine setup, linker, host imports, preopens, seccomp | [weft-runtime main.rs](../../../crates/weft-runtime/src/main.rs) |
| Host WIT (`weft:app@0.1.0`: notify, ipc, fetch, notifications, clipboard) | [weft-app.wit](../../../crates/weft-runtime/wit/weft-app.wit) |
| Demo copies of the WIT | `examples/*/wit/deps/weft-app/weft-app.wit` |
| Capability strings accepted at package check | `is_known_capability` and its `EXACT` list in [weft-pack](../../../crates/weft-pack/src/main.rs) |
| Capability to preopen mapping at launch | [appd runtime.rs](../../../crates/weft-appd/src/runtime.rs) |
| File portal | [weft-file-portal](../../../crates/weft-file-portal/src/main.rs) |

## Working rules

- Features: `wasmtime-runtime` enables the engine, `net-fetch` the fetch import,
  `seccomp` the syscall blocklist. Without `wasmtime-runtime` the binary prints
  READY and exits; that path is a limited build fixture, never product
  evidence.
- Verify Wasmtime and wasmtime-wasi behavior in the version `Cargo.lock`
  selects, including how the linker treats differing WIT patch versions. A
  textual version difference is not an ABI defect until an actual component
  shows it.
- Every advertised import checks the effective grant on every call. Absent or
  unsupported operations fail explicitly; they are not successful no-ops.
- Keep read-only and read/write access distinct through preopens and the
  portal. Confinement uses descriptor-relative or equivalent kernel-enforced
  operations; lexical prefix checks and `canonicalize()` followed by reopening
  are not sufficient against symlinks and replacement races.
- Fetch needs explicit schemes, destinations, methods, deadlines, size limits
  and redirect authorization; keep HTTP errors distinct from transport errors.
- Keep one canonical WIT source with a checked synchronization mechanism for its
  copies.
- Do not describe the seccomp blocklist or process separation as complete
  confinement.

## Evidence

`python .agents/scripts/verify.py portable runtime` covers the stub and the
engine-enabled build, lints and unit tests. It does not instantiate a component.
Host import behavior needs an actual component fixture for the allowed, denied
and failure paths of each import; confinement needs traversal, symlink and
race cases against the real portal and preopens. No profile or CI job compiles
the `seccomp` feature; check it on Linux with
`cargo clippy -p weft-runtime --features wasmtime-runtime,seccomp --all-targets --locked -- -D warnings`
and prove filtering with a component or process fixture, not by compilation.
Rebuild demo components with the `demos` profile when the WIT changes.
