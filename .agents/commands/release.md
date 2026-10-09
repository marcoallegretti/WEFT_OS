---
name: release
description: Prepare a 0.0.x or 0.1.0 candidate's evidence and recommend promotion without promoting it automatically.
---

# Prepare a release stage

Input: the milestone scope, the target version and stage, and the exact
candidate commit. Read [versions and releases](../../AGENTS.md#13-versions-and-releases),
the blueprint's [version progression](../../BLUEPRINT.md#121-version-progression)
and, for stable `0.1.0`, the [stable release gate](../../BLUEPRINT.md#14-stable-release-gate).

1. **Inventory.** List existing tags, published releases and the versions in
   `Cargo.toml`, crate manifests and package manifests. Choose the next unused
   `0.0.x` for an accepted milestone scope; never reuse, move or rewrite a
   published tag or release. Record how the unreleased workspace value `0.1.0`
   is reconciled.
2. **Check the stage contract.** `alpha.N` needs defined scope, ownership and
   fixtures with incompleteness stated. `beta.N` needs the declared scope
   implemented, relevant integration and failure paths passing and interfaces,
   support scope and budgets frozen. `rc.N` needs the exact candidate complete
   for its scope, the required matrix passing, independent technical review and
   ready artifacts and migration behavior. Final, released as the plain version
   without a `-final` suffix, needs every scoped promise to pass.
3. **Collect candidate evidence** for that exact commit with [verify](verify.md):
   every applicable runner profile plus the blueprint fixtures and target
   environments in scope. Record commit, environment, features, commands, results
   and artifact digests. A missing environment leaves the gate incomplete.
4. **Obtain independent technical review** of the candidate with the
   [reviewer](../agents/reviewer.md), aimed at showing it is not ready. A self
   review does not satisfy a release gate; an unavailable reviewer leaves the gate
   pending while other authorized work continues.
5. **Resolve or scope.** Blocking findings prevent promotion. Corrections after
   review invalidate the affected evidence: rebuild the candidate, rerun the
   affected checks and re-review. Unsupported features are excluded explicitly,
   not left as silent success paths.
6. **Prepare release notes** for maintainers: scope, support matrix, known
   limitations, compatibility and migration notes, commit and artifact digests,
   and the evidence actually executed.
7. **Recommend.** Return the evidence, open findings and a recommendation.
   Promotion, tagging and publication happen only under granted release
   authority, and publish the artifact that was tested. If final metadata
   requires a rebuild, verify that rebuilt artifact before publishing it.
