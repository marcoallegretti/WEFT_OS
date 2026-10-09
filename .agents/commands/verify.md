---
name: verify
description: Map a change to the evidence that proves it, run the applicable checks and report actual results and gaps.
---

# Select and run the evidence

Input: accepted scope and a diff or explicit base/head range. Read the
[proof rules](../../AGENTS.md#6-proof-before-implementation), the blueprint's
[verification profiles](../../BLUEPRINT.md#112-verification-profiles) and
[acceptance fixtures](../../BLUEPRINT.md#113-required-acceptance-fixtures), and
[CI](../../.github/workflows/ci.yml).

List the observable behaviors the change affects, not the files. For each,
name the check that would fail if it were wrong and classify it as direct proof,
incidental coverage or missing. Then select:

| Affected area | Runner profiles | Evidence the runner cannot provide |
|---|---|---|
| Portable crates (`weft-appd`, `weft-pack`, `weft-ipc-types`, `weft-file-portal`, `weft-mount-helper`, `weft-build-meta`, `weft-runtime` stub) | `portable` | Child processes, sockets and filesystems exercised for real where the change depends on them |
| `weft-compositor` | `portable linux` | Nested winit session with real clients; DRM session on the declared target |
| Servo hosts | `linux servo-embed` | Painted and presented pixels, input, resize and idle wakeup on a running compositor |
| `weft-runtime` engine or host imports | `portable runtime` | Actual component fixtures for allowed, denied and failure paths; a Linux build of the `seccomp` feature when it is touched |
| WIT, demo components | `demos packages` | Instantiating the rebuilt component under `weft-runtime` |
| Package tooling, signing, demo packages | `portable packages` | Tamper, interruption and data-retention fixtures |
| Dependency or fork change | Every profile whose crates resolve the changed package | Resolved graph inspection; the rendered or runtime behavior the change claims |
| Nix, systemd, VM | None directly | `infra/vm/build.sh` and a booted graphical image |
| Contributor documentation and toolkit | `toolkit` | Manual consistency with `AGENTS.md` and the blueprint |

Run the selected profiles with `python .agents/scripts/verify.py <profiles>`, or
the equivalent commands directly, and record actual exit results. Run them
against the committed graph, without local `[patch]` overrides.

A missing native library, toolchain target, display, GPU, VM or Nix installation
is unavailable evidence: name it and the boundary left unverified. Never count a
skip, a feature-disabled build or a mocked boundary as proof of the real path.
Choose expensive checks by risk; do not repeat unrelated ones once the risk is
resolved. Release gates need the complete matrix in [release](release.md).

Return a compact table of behaviors, proof and status; the commands actually run
with results; remaining gaps; and the single most valuable missing check if one
exists. Do not change product code to probe coverage unless that experiment is
authorized and isolated.
