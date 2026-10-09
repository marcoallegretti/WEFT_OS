---
name: maintenance
description: Run one maintenance role for one qualifying problem from the verified current main, isolated from primary development.
---

# Run one maintenance task

Input: one named role, [architecture-debt-scout](../agents/architecture-debt-scout.md),
[continuous-simplicity](../agents/continuous-simplicity.md) or
[continuous-refactoring](../agents/continuous-refactoring.md), and optionally a
candidate problem. Read [maintenance](../../AGENTS.md#8-maintenance) in the
contract and load the role.

1. **Verify the base.** Fetch `main` and record its revision. If freshness cannot
   be verified, state the revision you used and the limitation. Never switch,
   reset or edit the active development checkout.
2. **Apply the threshold.** Find the strongest problem that has a named affected
   party, present repository evidence, present cost and an independent acceptance
   criterion. A problem inside the active goal's acceptance criterion belongs to
   that goal, not here. If nothing qualifies, report that and stop.
3. **Check for existing work.** Search issues and pull requests for the same
   underlying problem and update the existing one instead of duplicating. Report
   incomplete duplicate checking when access is unavailable.
4. **Record it.** Draft the issue with [scope](scope.md) and the
   [maintenance form](../../.github/ISSUE_TEMPLATE/maintenance.yml), creating or
   updating it only within granted authority. The scout stops here.
5. **Isolate.** For simplicity or refactoring, add a worktree from the fetched
   `main`, for example `git worktree add ../weft-share-package-resolution -b refactor/share-package-resolution origin/main`.
   Use a branch name that describes the change, not the role.
6. **Change and review.** Follow [implement](implement.md) from proof to one
   reviewed pull request, or to a prepared local result when publication is not
   authorized. If the change turns out to need a
   [decision](../../AGENTS.md#9-decisions-and-authority), stop it and record
   the decision needed.
7. **Stop.** One run handles one problem. Do not continue collecting
   opportunities, and do not merge.
