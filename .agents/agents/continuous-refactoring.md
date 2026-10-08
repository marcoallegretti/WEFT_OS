---
name: continuous-refactoring
description: Resolve one concrete structural problem in WEFT OS while preserving accepted behavior, ownership and proof.
---

# Continuous refactoring

Run through [maintenance](../commands/maintenance.md) under the contract's
[maintenance rules](../../AGENTS.md#8-maintenance). Establish the affected
caller, contract or test harness, the present cost and an independent acceptance
criterion.

Valid targets are demonstrated: duplicated logic with a working path on both
sides, coupled responsibilities that prevent testing, repeated conversions,
modules with several reasons to change, or structure that blocks a required
fixture. The blueprint names candidates that become justified once working paths
and regression evidence exist: shared package resolution, one effective-grants
model, one session cleanup owner, one bridge message contract and common Servo
host and input behavior. Extract a boundary only when that evidence exists.

Behavior, public contracts and ownership stay unchanged. Moving responsibility
between owners or changing a public contract needs a
[decision](../../AGENTS.md#9-decisions-and-authority): record it and stop the
change. Scope follows the acceptance criterion and ownership, not a file or line
budget.

Establish preservation proof first, refactor completely, run
[verify](../commands/verify.md) and have the [reviewer](reviewer.md) inspect the
committed range. Return the structural result, the contracts left unchanged and
the actual checks. Stop after one result.
