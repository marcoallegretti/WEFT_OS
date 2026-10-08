---
name: scope
description: Establish a reported problem or proposal and draft verifiable scope with owner, acceptance criterion and proof.
---

# Establish the scope

Input: a defect report, desired behavior or proposal. Read
[AGENTS.md](../../AGENTS.md), the relevant sections of
[BLUEPRINT.md](../../BLUEPRINT.md) and the matching
[skills](../README.md#skills). The result is a draft; this procedure neither
implements it nor authorizes publication.

1. **Inspect.** Read the implemented source, the public contract involved
   (manifest, WIT, shell protocol, IPC types) and the selected dependency source
   when an external API matters. Check whether the blueprint already records the
   problem, for example in its [baseline findings](../../BLUEPRINT.md#32-findings-that-determine-the-implementation-order).
2. **Reproduce** where practical, with the exact commit, environment, features
   and command. Separate observations from assumptions and say which level of
   [evidence](../../BLUEPRINT.md#111-evidence-levels) each observation reached.
3. **Name the owner** from the
   [ownership table](../../BLUEPRINT.md#4-architecture-and-ownership) and the
   affected user, caller or contract. If nobody is worse off today, stop: there
   is no issue.
4. **Check for duplicates** among existing issues and pull requests. If access is
   unavailable, say that duplicate checking is incomplete.
5. **Decide the path.** Ordinary work within accepted architecture becomes a
   change. A material departure from accepted direction, compatibility, trust,
   support scope or data policy needs a
   [decision](../../AGENTS.md#9-decisions-and-authority): prepare the evidence,
   alternatives and trade-offs and mark implementation as waiting for it.
6. **Draft** against the matching form:
   [defect or change](../../.github/ISSUE_TEMPLATE/change.yml),
   [maintenance](../../.github/ISSUE_TEMPLATE/maintenance.yml) or
   [decision](../../.github/ISSUE_TEMPLATE/decision.yml).

The draft states observed and expected behavior, evidence with file and line
references, owner and affected boundary, present impact, one acceptance
criterion, the proof that would fail today and pass afterwards, and relevant
non-goals. A claim without a verification surface is unfinished scope.

Return the draft plus any missing evidence or decision. Create or update an
issue only when that is part of the authorized task.
