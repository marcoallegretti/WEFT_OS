---
name: weft-integration-verification
description: Use for end-to-end and visual evidence in WEFT OS — real sessions, pixels and native input, failure injection, reliability runs and evidence records.
---

# Integration and visual verification

Integration evidence shows what unit tests cannot: real frames, native input,
real child processes, real components and an installed system. Read the
blueprint's [evidence levels](../../../BLUEPRINT.md#111-evidence-levels),
[verification profiles](../../../BLUEPRINT.md#112-verification-profiles),
[acceptance fixtures](../../../BLUEPRINT.md#113-required-acceptance-fixtures) and
[user journeys](../../../BLUEPRINT.md#22-required-user-journeys).

## Fixture design

- Choose the fixture before implementing. It must fail when the promised
  behavior breaks and must not restate the implementation or only grep a success
  log.
- For rendering, use a deterministic page with known color regions, text,
  scrolling content, a text field, an interactive counter and a controlled
  animation. Compare actual pixels read back after painting at the relevant
  scales and sizes, including first frame, resize and idle wakeup.
- For input, drive the real native path on a running compositor: key press and
  release, modifiers, declared US and Italian layouts with non-ASCII text,
  buttons, motion, wheel, focus changes and scale changes. Check for stuck input
  after focus loss and client exit.
- For lifecycle, inject failures at each startup phase and after `Running`:
  missing binary, invalid package, renderer failure, runtime trap, crash,
  cancellation and timeout. Assert a terminal reason and that no owned process,
  socket, relay or mount remains.
- For data, use Notes with empty, non-ASCII, multi-line, quoted and literal
  backslash text, delayed replies, failed writes and competing writers; repeat
  after close, daemon restart, update and reboot.
- Counter and Notes are conformance applications: their happy paths are
  necessary but insufficient.

## Running and recording

The [runner](../../scripts/verify.py) provides only build, lint, unit and
package checks. Integration fixtures run in the environment they claim: a nested
winit session for nested-desktop evidence, a DRM session on the declared target,
the booted reference image for installed behavior. Keep captures and logs under
ignored paths such as `target/`; commit fixtures, not their outputs.

Record for each run the commit, environment (host, compositor backend, GPU or
virtual device, Mesa, kernel), enabled features, exact command, observed result
and diagnostics. A screenshot shows pixels, not input routing or accessibility.
An unavailable display, GPU or VM leaves the gate incomplete; say which boundary
remained unverified.

Reliability qualification follows the blueprint targets: repeated clean boots,
launch and close cycles without orphans, mixed interactive and idle runs.
Performance and budget claims need measurements on the reference target.
