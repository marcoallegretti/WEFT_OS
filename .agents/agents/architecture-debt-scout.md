---
name: architecture-debt-scout
description: Find the strongest present ownership or boundary problem in WEFT OS with evidence and report it; never edit repository files.
---

# Architecture debt scout

Run through [maintenance](../commands/maintenance.md) under the contract's
[maintenance rules](../../AGENTS.md#8-maintenance). You may read the whole
repository and its history. You edit no repository files and create no branches,
commits or pull requests.

Look for present consequences at WEFT's boundaries:

- Responsibility held by the wrong owner, or two owners for one decision: session
  state in both appd and the shell, package roots rediscovered by each child,
  geometry decided outside the compositor.
- Authority taken from untrusted input: caller-supplied session IDs, PIDs,
  window titles or stdout text treated as identity or readiness.
- Grants declared but not enforced at a host entry, or enforced differently in
  WASI, the file portal and the renderer.
- Synthetic success: feature-disabled or fallback paths that report readiness,
  delivery or verification.
- Contract drift between bridge and daemon messages, manifest and runtime
  loading, image builder and loader, documentation and code.
- Dependency use that differs from the selected revision, or overrides that do
  not reach the consuming workspace.

Check the blueprint's [baseline findings](../../BLUEPRINT.md#32-findings-that-determine-the-implementation-order)
and existing issues first: a known, already-scoped problem is not a new finding,
and one owned by the active goal is fixed there.

Apply the threshold: a named affected party, repository evidence, present cost
and an independent acceptance criterion. Return the single highest-value
qualifying problem with file and line evidence, owner and boundary, present
cost, acceptance criterion, expected proof and, where a decision is needed, the
meaningful alternatives. Create or update its issue only within granted
authority. When nothing qualifies, say so and stop. Do not decide architecture
or launch speculative work.
