# Security Model

## Principles

WEFT OS enforces a capability-based security model. No capability is granted by default; all capabilities are declared in `wapp.toml` and verified before an app runs.

## Capability Verification

`weft-pack check` validates capability strings against a known set (`KNOWN_CAPS`) before installation. Unknown capability strings are rejected. The validated capability list is read by `weft-appd` at session start to map capabilities to concrete resource grants.

## Process Isolation

Each app session runs as a separate OS process (`weft-runtime`). When systemd is available, the process is wrapped in a systemd scope (`weft-apps.slice`) with `CPUQuota=200%` and `MemoryMax=512M`.

## Filesystem Isolation

Apps access the filesystem only through WASI preopened directories. Each capability maps to a specific host path preopened at a fixed guest path. The `weft-file-portal` process enforces path allowlists and blocks `..` traversal for apps that use the portal protocol.

## Package Signing

Packages are signed with Ed25519 (`ed25519-dalek`). The signature covers the SHA-256 hash of `wapp.toml` and `app.wasm`. `weft-pack verify` checks the signature before installation.

For verified read-only package storage, `weft-pack build-image` produces an EROFS image protected with dm-verity. Mounting requires the setuid `weft-mount-helper` which calls `veritysetup`.

## Seccomp

`weft-runtime` supports an optional seccomp BPF filter (compiled in with `--features seccomp`). The filter blocks a set of dangerous syscalls: `ptrace`, `process_vm_readv`, `process_vm_writev`, `kexec_load`, `mount`, `umount2`, `setuid`, `setgid`, `chroot`, `pivot_root`, `init_module`, `finit_module`, `delete_module`, `bpf`, `perf_event_open`, `acct`. All other syscalls are allowed; the policy is permissive with a syscall blocklist, not a strict allowlist.

## Wayland Surface Isolation

Each app registers its surface with the compositor via `zweft_shell_manager_v1`. The compositor enforces that each surface belongs to the session that created it. The app cannot render outside its assigned surface slot.

## JavaScript Engine — GAP-6

The Servo embedding uses SpiderMonkey as its JavaScript engine. SpiderMonkey is a complex JIT compiler. The following are known limitations that are not mechanically addressed by WEFT OS at this time:

- SpiderMonkey is not sandboxed at the OS level beyond what the Wasmtime/Servo process isolation provides.
- JIT-compiled JavaScript runs with the same memory permissions as the rest of the Servo process.
- SpiderMonkey vulnerabilities (CVE-class bugs in the JIT or parser) would affect the isolation boundary.

**Current mitigation:** the `weft-app-shell` process runs as an unprivileged user with no ambient capabilities. The seccomp filter blocks the most dangerous privilege-escalation syscalls when enabled. WEFT does not claim stronger JavaScript engine isolation than Gecko/SpiderMonkey itself provides.

**Not addressed:** JIT spraying, speculative execution attacks on SpiderMonkey's JIT output, and parser-level memory corruption bugs. These require either a Wasm-sandboxed JS engine or hardware-enforced control-flow integrity, neither of which is implemented.

This gap (GAP-6) is tracked. The bounded statement is: *WEFT OS relies on SpiderMonkey's own security properties for the JavaScript execution boundary. Any SpiderMonkey CVE that allows code execution within the renderer process is in-scope for the WEFT OS threat model.*
