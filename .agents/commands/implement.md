---
name: implement
description: Carry accepted scope through proof, implementation, validation and diff review to one atomic result.
---

# Implement accepted scope

Input: an issue or an explicitly accepted task with an acceptance criterion.
Read [AGENTS.md](../../AGENTS.md) and the matching [skills](../README.md#skills).
This procedure applies the [reviewed change loop](../../AGENTS.md#5-reviewed-change-loop)
to contribution work and to each step of primary development. It adds no
approval layer: in primary development, continue to the next accepted step after
each accepted result.

1. **Inspect and confirm scope.** Verify the problem against current source and
   evidence. If the acceptance criterion or proof is missing, use
   [scope](scope.md). An issue is not a decision; if one is required and
   unresolved, prepare it and work on something independent.
2. **Prepare the branch.** Inspect `git status`, the current branch, worktrees and
   remotes. Fetch `main` and branch from it, or from the unmerged branch the work
   depends on, with a descriptive
   [branch name](../../AGENTS.md#14-branches-commits-and-publication). Preserve
   unrelated changes; never commit contribution work directly to `main`.
3. **Review the plan.** Name the owner, the boundary touched, compatibility
   promises affected and the proof. Check uncertain APIs in the dependency
   source selected by the lockfile. Fix it here if the design crosses ownership.
4. **Establish proof.** Reproduce a defect first. For new behavior, add the
   fixture or contract case that fails without the change. Identify which
   evidence needs a real environment (compositor, renderer, VM) and whether it is
   available.
5. **Implement** the smallest complete change. Keep manifests, WIT, protocol,
   IPC types, documentation and callers coherent. No synthetic success paths.
6. **Validate** with [verify](verify.md): the smallest sufficient profiles plus
   any boundary-specific evidence. Fix failures within scope. Explain every
   changed expectation; never re-bless output to make it pass.
7. **Review the actual diff** with the [reviewer](../agents/reviewer.md), from a
   reviewer who did not write it when available, otherwise as a fresh separate
   pass whose limitation you state. Review `git diff <base> <head>` for commits,
   or both staged and unstaged diffs before committing.
8. **Correct, revalidate and re-review** until no blocking finding remains.
   Recheck after material corrections, rebases and conflict resolutions.
9. **Commit atomically.** Inspect `git status` and the complete diff; exclude
   generated, transient and local-environment files (for example lockfile changes
   caused by local `[patch]` overrides). Write the message for maintainers.
10. **Publish within authority.** If pushing or opening a pull request is
    authorized and available, fill the
    [pull request template](../../.github/pull_request_template.md) and inspect
    automated review integrations first. Otherwise return the local result and
    the prepared description.
11. **Reassess** the goal from the new repository state and continue with the
    next dependent step.

Report what changed, what proves it, the checks actually run with results, and
material limitations. A separately actionable problem found on the way goes
through the [maintenance threshold](../../AGENTS.md#8-maintenance) instead of
the pull request body. Completing a change grants no merge or release authority.
