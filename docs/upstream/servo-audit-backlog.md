# Servo Audit Backlog

This backlog maps WEFT requirements to the upstream audit and contribution work needed before WEFT can rely on Servo for system-shell and app-UI duties.

## Rules

- No item is marked resolved without upstream evidence.
- A local workaround does not close an upstream dependency gap.
- Every audit item must end in one of: verified, upstream issue, local design adjustment, or blocked.

## Backlog

| Area | WEFT requirement | Current status | Next action | Exit condition |
| --- | --- | --- | --- | --- |
| Wayland input | Shell-grade keyboard, pointer, touch, and IME behavior | Not verified in this repository | Audit Servo and winit behavior on Linux Wayland | Verified behavior or upstream issues filed for confirmed gaps |
| Wayland surface pipeline | Correct damage tracking, presentation timing, and buffer path behavior | Not verified in this repository | Audit Servo -> WebRender -> wgpu -> Wayland path | Explicit verification or upstream issue set |
| WebGPU completeness | Stable GPU app rendering path on Linux Mesa | Not verified in this repository | Compare Servo WebGPU coverage to WEFT needs | Verified required subset or upstream issue set |
| Multiprocess isolation | App UI crash isolation from shell UI | Known to be incomplete as a WEFT assumption | Audit Servo multiprocess and content-process behavior | Written gap assessment with next action |
| JavaScript runtime stability | Sufficient stability for system-shell and app UI JavaScript | Not verified in this repository | Audit current Servo main behavior and issue tracker | Verified risk record or upstream issue set |
| Accessibility | Linux AT-SPI readiness for shell and app UI | Not verified in this repository | Audit current AccessKit and Servo Linux path | Verified support state or upstream issue set |

## Local follow-up outputs expected from each audit

Each audit should produce:

- a reproducible environment description
- exact upstream revision or release under test
- observed result
- links to upstream issues if gaps are confirmed
- the WEFT planning consequence
