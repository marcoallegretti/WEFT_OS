# Linux Development Environment

WEFT OS is developed on a Windows workstation, but the authoritative runtime target for system work is a Linux VM or QEMU guest.

## Baseline target

Use a recent Linux distribution with:

- systemd as PID 1
- Wayland-capable graphics stack
- Mesa userspace drivers
- a recent Rust toolchain compatible with `rust-toolchain.toml`

## Purpose of the guest environment

The guest is the validation target for:

- systemd service assumptions
- Wayland compositor bring-up
- Servo Wayland client behavior
- Wasmtime runtime supervision assumptions

## Host versus target boundary

The Windows host is acceptable for editing, documentation, and workspace validation.

The Linux guest is authoritative for:

- graphics stack behavior
- compositor and shell startup order
- systemd unit behavior
- process supervision assumptions tied to Linux userspace

## Minimum guest setup goals for the next implementation wave

- install Rust toolchain matching `rust-toolchain.toml`
- install build essentials needed by Rust crates in this repository
- confirm `cargo fmt`, `cargo clippy`, and `cargo test` run successfully
- prepare a repeatable guest definition for future compositor and shell work

## Current status

This repository does not yet automate guest provisioning. That work should begin only after the foundational workspace and design documents are stable.
