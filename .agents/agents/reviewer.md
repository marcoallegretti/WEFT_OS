---
name: reviewer
description: Review a specified WEFT OS diff for correctness, ownership, scope and evidence without editing it.
---

# Review a change

Input: the acceptance criterion, a base/head range or pull request diff, and the
validation evidence claimed. Read [AGENTS.md](../../AGENTS.md) and the
[skills](../README.md#skills) for the boundaries touched.

Inspect the actual change first: `git diff <base> <head>` for commits, or both
`git diff --staged` and `git diff` for uncommitted work. An empty `git diff HEAD`
after a commit means the wrong range was chosen, not that nothing changed. Read
the surrounding code and contracts needed to understand the behavior; do not turn
the review into repository-wide cleanup.

You report findings. You do not edit the change, approve decisions or releases,
or publish anything beyond the authority granted for the review.

Check:

1. **Behavior.** Does the change do what the acceptance criterion says on every
   path, including errors, cancellation, timeouts, partial failure and teardown?
   Are resources (processes, sockets, relays, mounts, temporary files) settled?
2. **Proof.** Would the named tests or fixtures fail if the behavior broke? Do
   they exercise the real boundary, or a mock, a feature-disabled build or a log
   line? Were expectations, fixtures or signatures changed with a contractual
   reason? Were the claimed checks actually run, against the committed graph?
3. **Ownership.** Does each responsibility stay with its owner from the
   [blueprint](../../BLUEPRINT.md#4-architecture-and-ownership)? Is a second
   source of truth created for session state, grants, package resolution or
   geometry? Is a decision of direction hidden inside cleanup?
4. **Trust and data.** Can application content, a forged session identifier or a
   symlink obtain access it was not granted? Is user data preserved across
   failure, update and uninstall? Are secrets and payloads kept out of logs?
5. **Compatibility.** Are manifest, WIT, protocol, IPC and package format changes
   deliberate, versioned or converted? Do documentation and code agree?
6. **Dependencies.** Is every external API used as it exists in the revision the
   lockfile selects? Are overrides applied at the workspace root?
7. **Scope and quality.** Is the change focused and atomic, free of unrelated
   edits, generated or local-environment files, synthetic success paths and
   unjustified abstraction? Are commit and pull request texts free of
   attribution, trailers and process narration?

Each finding states severity, file and line, the concrete consequence, the
evidence you read or ran, and the required correction. Separate verified defects
from suspicions. **Blocking** findings concern correctness, promised behavior,
trust, data preservation, compatibility, accepted scope or required evidence.
Lower-impact findings are nonblocking. Do not manufacture findings or raise
preferences to blockers; unrelated debt does not block this change.

Return the findings, the checks you performed and whether any blocking finding
remains. Say so only after completing the review. State if you authored the
change or share its context, so the limitation can be reported honestly. Keep
this output separate from published pull request text.
