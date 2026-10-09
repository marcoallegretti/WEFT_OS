# Security Model

## Principles

WEFT OS enforces a capability-based security model. No capability is granted by default; all capabilities are declared in `wapp.toml` and verified before an app runs.

## Capability Verification

`weft-pack check` validates capability strings against the shared vocabulary in `weft_ipc_types::capability`. Unknown capability strings are rejected. At launch, `weft-appd` derives the session's grants from the manifest and refuses to start a package with a capability that is unknown, unsupported or unsatisfiable. `weft-runtime` enforces the grants it is given: read-only directories are preopened read-only, and each host import checks its grant on every call. See the capability table in `architecture.md`.

Installing a package does not yet ask the user to approve its capabilities; every declared, supported capability is granted at launch.

## Process Isolation

Each app session runs as a separate OS process (`weft-runtime`). When systemd is available, the process is wrapped in a systemd scope (`weft-apps.slice`) with `CPUQuota=200%` and `MemoryMax=512M`.

## Filesystem Isolation

Apps access the filesystem only through WASI preopened directories. Each capability maps to a specific host path preopened at a fixed guest path. The `weft-file-portal` process applies the same directory grants and access modes, but checks paths lexically: it blocks `..` traversal, not symbolic links. Components cannot currently reach its socket, since the runtime grants them no socket access.

## Package Signing

Packages are signed with Ed25519 (`ed25519-dalek`). The signature covers the SHA-256 hash of `wapp.toml` and `app.wasm`. `weft-pack verify` checks the signature before installation.

For verified read-only package storage, `weft-pack build-image` produces an EROFS image protected with dm-verity. Mounting requires the setuid `weft-mount-helper` which calls `veritysetup`.

## Seccomp

`weft-runtime` supports an optional seccomp BPF filter (compiled in with `--features seccomp`). The filter blocks a set of dangerous syscalls: `ptrace`, `process_vm_readv`, `process_vm_writev`, `kexec_load`, `mount`, `umount2`, `setuid`, `setgid`, `chroot`, `pivot_root`, `init_module`, `finit_module`, `delete_module`, `bpf`, `perf_event_open`, `acct`. All other syscalls are allowed; the policy is permissive with a syscall blocklist, not a strict allowlist.

## Wayland Surface Isolation

Each app shell is given exactly one Wayland connection, created for its session. weft-appd creates a connected socket pair, gives one end to the app shell as its standard input with `WAYLAND_SOCKET=0` and no `WAYLAND_DISPLAY`, and sends the other end to the compositor over the compositor IPC socket (`SCM_RIGHTS`) with the session ID and app ID. The compositor adds that end as a client bound to the session; the binding comes from appd holding the connection, never from an ID or process ID the client supplies. A session's `zweft_shell` windows carry the session's app ID whatever the client requests, and the shell-only `panel` and `overlay` roles are refused with a protocol error; layer-shell surfaces and the input-method interface are not offered to session clients at all. When the session ends, the compositor closes its client; when the client disconnects, the compositor reports it to appd; when appd's connection ends, the compositor closes every session client appd attached. The compositor accepts one appd connection at a time. A launch is refused (503) while appd is not connected to the compositor, rather than starting an app that could never show a window; if the connection drops between that check and the app shell's start, the session is stopped before the app shell starts. Frames for appd that its socket cannot take at once are queued and retried; a backlog beyond 4 MiB, from an appd that stopped reading, ends the connection and with it the sessions' clients.

Not yet covered: the compositor IPC socket accepts any process of the user while no appd is connected, so the first connection after a restart is trusted as appd; window placement, stacking, focus and close are not yet decided by the compositor per session (all toplevels start at the origin); and the trusted system shell is identified only by connecting through the display socket, which any process of the user, including an app shell, can open.

## JavaScript Engine (SpiderMonkey)

The Servo embedding uses SpiderMonkey as its JavaScript engine. SpiderMonkey is a complex JIT compiler. The following are known limitations that are not mechanically addressed by WEFT OS at this time:

- SpiderMonkey is not sandboxed at the OS level beyond what the Wasmtime/Servo process isolation provides.
- JIT-compiled JavaScript runs with the same memory permissions as the rest of the Servo process.
- SpiderMonkey vulnerabilities (CVE-class bugs in the JIT or parser) would affect the isolation boundary.

**Current mitigation:** the `weft-app-shell` process runs as an unprivileged user with no ambient capabilities. The seccomp filter blocks the most dangerous privilege-escalation syscalls when enabled. WEFT does not claim stronger JavaScript engine isolation than Gecko/SpiderMonkey itself provides.

**Not addressed:** JIT spraying, speculative execution attacks on SpiderMonkey's JIT output, and parser-level memory corruption bugs. These require either a Wasm-sandboxed JS engine or hardware-enforced control-flow integrity, neither of which is implemented.

The bounded statement is: *WEFT OS relies on SpiderMonkey's own security properties for the JavaScript execution boundary. Any SpiderMonkey CVE that allows code execution within the renderer process is in-scope for the WEFT OS threat model.*
