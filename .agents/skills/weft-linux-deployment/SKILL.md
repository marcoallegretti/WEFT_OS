---
name: weft-linux-deployment
description: Use for the Nix flake and reference VM, systemd units, native dependencies, installed paths, endpoints and service activation.
---

# Linux deployment

Owner: Linux and systemd own the seat, session substrate, service lifecycle and
process and resource boundaries. The installed closure must carry the real
feature-enabled product. Read the blueprint's
[release profile](../../../BLUEPRINT.md#21-required-release-profile),
[installed session](../../../BLUEPRINT.md#102-installed-session) and
[developer workflow](../../../BLUEPRINT.md#103-diagnostics-and-developer-workflow).

## Where things live

| Concern | Source |
|---|---|
| Flake, toolchain, dev shell | [flake.nix](../../../flake.nix), [flake.lock](../../../flake.lock) |
| Package derivations and features | [weft-packages.nix](../../../infra/nixos/weft-packages.nix) |
| VM configuration | [configuration.nix](../../../infra/nixos/configuration.nix) |
| VM build and run | [build.sh](../../../infra/vm/build.sh), [run.sh](../../../infra/vm/run.sh) |
| Service units | [infra/systemd](../../../infra/systemd/weft-appd.service) |
| Native build dependencies | [building.md](../../../docs/building.md), [CI](../../../.github/workflows/ci.yml) |
| Environment variables | [architecture.md](../../../docs/architecture.md) and the `std::env` reads in each crate |

## Working rules

- Keep one authoritative Nix and systemd configuration; make development units
  and scripts consistent with it.
- The closure includes the feature-enabled binaries (`servo-embed` for both
  shells, `wasmtime-runtime` for the runtime), the shell HTML and UI assets at
  installed paths, reference packages with an explicit trust configuration, the
  binary paths appd needs, and explicit activation and dependencies.
- Pass the real compositor display and daemon endpoints; never guess names such
  as `wayland-1`. Keep persistent data and ephemeral runtime storage separate.
- Fill fixed-output hashes, including Servo's git `outputHashes`, from an actual
  build, never by copying an unrelated value.
- Never fabricate `pkg-config` files or library versions to satisfy a build.
  Repair the environment or exclude the feature explicitly. Do not commit
  machine-specific usernames or checkout paths.
- Record the exact VM machine, graphics device, kernel, Mesa, CPU and memory
  used for evidence. A headless VM or a successful evaluation is not graphical
  acceptance.

## Evidence

The runner has no Nix or VM profile. Build with `bash infra/vm/build.sh` and run
with `bash infra/vm/run.sh` where Nix with flakes is available, and record the
environment. Installed-system claims need a fresh graphical boot that starts the
complete system without checkout paths or manual repair, plus persistence across
reboot and update. A missing Nix installation or virtualization support is
unavailable evidence.
