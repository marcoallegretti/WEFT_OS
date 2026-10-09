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

## Package Signing and Trust

Packages are signed with Ed25519 over their content digest: the SHA-256 of an inventory listing every file in the package except the root `signature.sig`, one `<path>\t<sha-256>` line per file sorted by path. Every file is covered, including the UI. Names must be UTF-8 without control characters, so no name can imitate other inventory lines. Packages may hold only directories and regular files; a symbolic link or special file makes `weft-pack check`, `sign`, `verify` and `install` refuse the package. The committed demo signatures verify under this rule.

`weft-pack install` admits a package only when its signature verifies with a key in the trust store: `*.pub` files (64 hex digits) in `$XDG_CONFIG_HOME/weft/trusted-keys` and `/etc/weft/trusted-keys`, or only in `$WEFT_TRUSTED_KEYS` when that is set. A malformed key file is an error, not skipped. An unsigned or untrusted package is refused unless the developer passes `--dev`, which installs it as development content. The package is copied into the store first and checked on that copy, which is then renamed into place, so the bytes that are verified are the bytes that are installed. The copy holds each source directory open and opens its entries relative to it, never following links, so replacing a directory with a link while the copy runs cannot redirect it. Each entry must keep the type, device and inode it had when it was examined just before it was opened, and a FIFO is never waited on. A device node hard-linked in place of a file in that interval is opened before it is refused; `fs.protected_hardlinks`, on by default, prevents such links to devices the attacker does not own. Installed files are set to 0644, or 0755 when the source is executable, and directories to 0755, whatever the umask, so no installed file is writable by other users. The app ID is read from the staged copy. Archives are unpacked into a fresh private directory in the store, which is always removed.

The first installation of an app ID records its owner in `$XDG_DATA_HOME/weft/owners/<id>`: the publisher key, or development. Installations and uninstallations of one ID under one data home hold a per-ID lock, so concurrent installations cannot leave a placed package without its record. Every later installation of that ID must have the same owner, so neither another trusted publisher nor a development build can take over an ID, and with it the app's data. The record survives uninstall while the app's data remains; uninstalling with no data left frees the ID. App data with no recorded owner, as left by installations from before owner records, is given to a new owner only with `--claim-data`. A record is written under a temporary name and linked into place, so it is never partial, and a record created by an installation that then fails to place its package is removed. A record left by an installation that was killed before placing its package is released by `weft-pack uninstall <id>` once no data remains. A record is only released while no known store (the user store, the system store and `$WEFT_APP_STORE`) holds the package; a package installed into another custom store is uninstalled with that store selected. The demo key in `examples/keys` is a test fixture, not a signing authority.

Not yet covered: `weft-appd` does not check signatures or owners at launch, so the installed directory is trusted as installed; key revocation, rotation and ownership transfer have no workflow; and capabilities are granted at launch without asking the user, independently of the signature.

For verified read-only package storage, `weft-pack build-image` produces an EROFS image protected with dm-verity. Mounting requires the setuid `weft-mount-helper` which calls `veritysetup`. The image's root hash is not yet authenticated (see `architecture.md`).

## Seccomp

`weft-runtime` supports an optional seccomp BPF filter (compiled in with `--features seccomp`). The filter blocks a set of dangerous syscalls: `ptrace`, `process_vm_readv`, `process_vm_writev`, `kexec_load`, `mount`, `umount2`, `setuid`, `setgid`, `chroot`, `pivot_root`, `init_module`, `finit_module`, `delete_module`, `bpf`, `perf_event_open`, `acct`. All other syscalls are allowed; the policy is permissive with a syscall blocklist, not a strict allowlist.

## Wayland Surface Isolation

Each app registers its surface with the compositor via `zweft_shell_manager_v1`. The compositor enforces that each surface belongs to the session that created it. The app cannot render outside its assigned surface slot.

## JavaScript Engine (SpiderMonkey)

The Servo embedding uses SpiderMonkey as its JavaScript engine. SpiderMonkey is a complex JIT compiler. The following are known limitations that are not mechanically addressed by WEFT OS at this time:

- SpiderMonkey is not sandboxed at the OS level beyond what the Wasmtime/Servo process isolation provides.
- JIT-compiled JavaScript runs with the same memory permissions as the rest of the Servo process.
- SpiderMonkey vulnerabilities (CVE-class bugs in the JIT or parser) would affect the isolation boundary.

**Current mitigation:** the `weft-app-shell` process runs as an unprivileged user with no ambient capabilities. The seccomp filter blocks the most dangerous privilege-escalation syscalls when enabled. WEFT does not claim stronger JavaScript engine isolation than Gecko/SpiderMonkey itself provides.

**Not addressed:** JIT spraying, speculative execution attacks on SpiderMonkey's JIT output, and parser-level memory corruption bugs. These require either a Wasm-sandboxed JS engine or hardware-enforced control-flow integrity, neither of which is implemented.

The bounded statement is: *WEFT OS relies on SpiderMonkey's own security properties for the JavaScript execution boundary. Any SpiderMonkey CVE that allows code execution within the renderer process is in-scope for the WEFT OS threat model.*
