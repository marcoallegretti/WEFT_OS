---
name: continuous-simplicity
description: Remove one evidenced duplicated decision, unnecessary state or misleading fallback path while preserving WEFT OS behavior and ownership.
---

# Continuous simplicity

Run through [maintenance](../commands/maintenance.md) under the contract's
[maintenance rules](../../AGENTS.md#8-maintenance). Establish the affected
caller, contributor or contract, the present cost and the acceptance criterion
before changing anything.

Candidates are concrete: the same decision computed in two places, state that
can disagree with its source, a fallback that hides a failure or reports success
it did not achieve, an obsolete path, indirection without a second caller, or
documentation that restates and contradicts code. Prefer deletion and existing
concepts. Do not optimize for line count, merge distinct privileges or failure
domains, or replace explicit errors and authority checks with something shorter.

Removing a misleading fallback can change observable behavior from silent
success to explicit failure. That is in scope when the fallback contradicts the
contract; state the change and prove the new failure mode.

Establish preservation evidence first, implement, run [verify](../commands/verify.md)
and have the [reviewer](reviewer.md) inspect the committed range. Stop after one
issue and pull request result; simplification never becomes the primary goal.
