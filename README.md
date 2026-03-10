# WEFT OS

WEFT OS is a Linux-based operating system effort built around a Smithay compositor, a Servo-rendered system shell, and a Wasmtime-based application runtime.

## Current repository scope

This repository currently contains:

- the baseline Rust workspace
- public engineering documentation derived from the authoritative blueprint
- initial design documents for the shell protocol boundary and the Wasm–Servo channel
- local and CI validation paths for repository bootstrap work

It does not yet contain a compositor, shell, or application runtime implementation.

## Source of truth

The authoritative technical reference for this repository is `docu_dev/WEFT-OS-COMPREHENSIVE-BLUEPRINT.md`.

Historical blueprint documents exist in `docu_dev/`, but they are not implementation authority where they conflict with the comprehensive blueprint.

## Privacy boundary

`docu_dev/` is a private coordination area used during development. It is intentionally ignored by git and is not part of the tracked public repository surface.

## Development model

- Primary development host: Windows workstation
- Primary runtime target: Linux VM or QEMU guest
- Core system language: Rust

## Validation

On Windows PowerShell:

```powershell
./infra/scripts/check.ps1
```

On Linux:

```bash
./infra/scripts/check.sh
```

## Repository layout

```text
crates/      Rust workspace members
docs/        Public engineering documentation
infra/       Validation scripts and VM workflow material
```
