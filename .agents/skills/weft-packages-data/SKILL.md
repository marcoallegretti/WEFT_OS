---
name: weft-packages-data
description: Use for wapp.toml manifests, weft-pack, signing and trust, installation and updates, app data storage and EROFS dm-verity images.
---

# Packages, trust, data and updates

Owner: package tooling owns manifest validation, content identity, trust,
staging, activation and retention. User data has its own lifecycle and is never
owned by installed package bytes. Read the blueprint's
[packages, trust, storage and updates](../../../BLUEPRINT.md#9-packages-trust-storage-and-updates)
and [Notes requirements](../../../BLUEPRINT.md#82-notes).

## Where things live

| Concern | Source |
|---|---|
| `check`, `info`, `install`, `uninstall`, `list`, `generate-key`, `sign`, `verify`, `bundle`, `unbundle`, `build-image`, `build-verity` | [weft-pack](../../../crates/weft-pack/src/main.rs) |
| Manifests | [counter wapp.toml](../../../examples/org.weft.demo.counter/wapp.toml), [notes wapp.toml](../../../examples/org.weft.demo.notes/wapp.toml) |
| Demo signing fixture keys | [examples/keys](../../../examples/keys/weft-sign.pub) |
| Data and package roots used at launch | [appd runtime.rs](../../../crates/weft-appd/src/runtime.rs) |
| Image mount and privileged helper | [appd mount.rs](../../../crates/weft-appd/src/mount.rs), [weft-mount-helper](../../../crates/weft-mount-helper/src/main.rs) |

## Working rules

- The implemented signature covers the recursively enumerated package inventory,
  including UI content and excluding the signature file, and both demo
  signatures verify under it. Preserve that compatibility deliberately when
  formalizing the format; do not re-sign demos to match a different description.
  Correct documentation that describes a narrower inventory.
- Demo keys are fixtures and never production signing authority. Release
  private keys stay outside the repository.
- Separate verified and development installation policies. Verified installs
  fail closed on invalid, missing, revoked or untrusted signatures; development
  installs are explicit and visibly marked.
- Validate IDs, entry containment, duplicates, link policy and the actual
  component before trust decisions. Bind each app ID and its retained data to a
  publisher key policy.
- Install and update through staging, validation, verification, immutable
  content and atomic activation. Failure preserves the previous active revision
  and user data; a live session stays pinned to its revision; uninstall keeps
  data unless an explicit purge is requested.
- Migrating the existing `apps/<id>/data` layout needs detection, backup, conflict
  handling and a test proving Notes contents survive. Keep it out of unrelated
  cleanups.
- Verified image mode never falls back to a mutable directory, and the privileged
  helper is enabled only after its caller authorization and failure paths are
  specified and tested.

## Evidence

`python .agents/scripts/verify.py portable packages` runs the package unit tests
and checks and verifies the committed demo packages. Trust, install and update
claims need tamper, interruption, publisher-ownership, active-update and
uninstall fixtures from the blueprint's
[acceptance list](../../../BLUEPRINT.md#113-required-acceptance-fixtures), and
persistence claims need reruns after reinstall, restart and reboot.
