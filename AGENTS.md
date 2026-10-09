# WEFT OS engineering contract

This is the single engineering contract for everyone who changes WEFT OS:
maintainers and external contributors. It governs scope, evidence, review, validation, branches and publication.

[`.agents/`](.agents/README.md) contains linked procedures, review roles, area
knowledge and a verification runner. Those files help apply this contract. They
do not define competing policy, grant permissions, schedule work, change the
active working mode or add approval steps. Local tool configuration may point
here; it must not restate, weaken or reinterpret these rules.

## 1. Sources of truth

| Question | Authority |
|---|---|
| Accepted product direction, ownership, support scope and completion criteria | [BLUEPRINT.md](BLUEPRINT.md) |
| Implemented behavior | Source code and the checks that actually executed against it |
| Public contracts | Package manifests (`wapp.toml`), [WIT](crates/weft-runtime/wit/weft-app.wit), the [shell protocol](protocol/weft-shell-unstable-v1.xml), [IPC types](crates/weft-ipc-types/src/lib.rs) and the versioned specifications that replace them |
| Selected dependency APIs | [Cargo.lock](Cargo.lock) and the demo lockfiles, at the exact selected revisions |
| Engineering process | This file |

The blueprint describes where WEFT is going. It is not evidence that a behavior
exists. Code that resembles a blueprint requirement does not satisfy it until
the requirement's acceptance evidence passes. Existing documentation, pin files
and version labels cannot establish behavior on their own; where they disagree
with source or executed checks, the documentation is wrong and needs correcting.

Before changing a public contract, inspect existing compatibility promises and
callers. Preserve compatibility deliberately or provide a tested conversion or
error path.

When evidence contradicts the blueprint, preserve its intent, establish why the
discrepancy exists and choose the maintainable evidence-backed solution. A
material change of direction follows [section 9](#9-decisions-and-authority).

## 2. Evidence

Never invent an API, protocol behavior, platform guarantee, capability or
measurement. Verify uncertain assumptions against the selected dependency
revision before relying on them: read the source of the crate version recorded
in the lockfile (Servo, Stylo, Smithay, Wasmtime, wasmtime-wasi, Tokio, winit,
surfman and others), not current upstream documentation or memory. Record the
revision used when the result matters to the change.

Research order: repository contracts and evidence, selected dependency source,
upstream documentation for that version, relevant standards or protocols,
comparable mature implementations, then a measured experiment.

The blueprint's [evidence levels](BLUEPRINT.md#111-evidence-levels) define what
each kind of check can establish. In particular:

- Compilation, type-checking and clippy prove that code builds, not that it
  renders, routes input, isolates, persists or recovers.
- Mocked sockets, DOMs, processes or compositors prove the logic exercised
  against the mock, not the real boundary.
- A capability string in a manifest, a host function registration or a
  frontend check does not prove enforcement.
- A success log line, `READY` message or exit code `0` from a feature-disabled
  build does not prove readiness of the product path.
- A skipped check, missing environment or unavailable device is an incomplete
  gate, never a pass.

Record the commit, environment, enabled features, exact command or harness,
result and useful diagnostics for evidence other contributors will rely on.
Distinguish tests that executed from tests that were only designed.

## 3. Ownership

Every change has an owner from the
[blueprint ownership table](BLUEPRINT.md#4-architecture-and-ownership). Identify
it before implementation. Do not let a component implicitly acquire
responsibility that belongs elsewhere:

- `weft-compositor` owns surfaces, geometry, focus, stacking, input routing and
  presentation.
- `weft-appd` owns package selection, effective grants, session identity, child
  supervision, readiness and cleanup.
- `weft-runtime` owns component instantiation and enforced host calls.
- `weft-servo-shell` and `weft-app-shell` render; they hold no second copy of
  session lifecycle and no authority the session did not grant.
- Package tooling owns validation, content identity, trust, staging and
  activation; it does not own user data.

Keep the UI and Wasm runtime in separate processes. Share code where behavior is
actually common, without merging privileges or failure domains. Do not replace
the compositor, browser engine or runtime as a shortcut around finishing their
contracts. Introduce a shared crate or abstraction only when it removes present
duplication or establishes a single source of truth.

## 4. Working modes

The modes below do not mix silently. Loading an `.agents/` file never switches
mode.

**Primary development** advances the blueprint under maintainer direction. A
goal may span many consecutive changes. At the start of a new goal, read the
blueprint and inspect the relevant source, manifests, tests, CI, lockfiles,
pins and Git state. Afterwards, reinspect the changed boundary and new evidence
for each step instead of repeating full discovery. Continue autonomously through
successive accepted changes while authorized independent work remains; do not
stop after a single patch or pull request. Fix related debt inside the active
acceptance criterion.

**Contribution** carries one issue or explicitly accepted scope through an
isolated branch to a pull request.

**Maintenance** handles exactly one qualifying problem that is unrelated to the
active goal, under [section 8](#8-maintenance). It never modifies the active
development checkout.

## 5. Reviewed change loop

Apply this loop to every workflow: primary development, fixes, simplification,
refactoring, dependency changes, build and packaging, documentation, governance
and release preparation.

1. **Inspect** current evidence and the implemented and accepted contract.
2. **State** the present problem, owner, bounded scope, observable acceptance
   criterion and the proof that will show it.
3. **Review scope** — ownership, proof and design — before changing the affected
   boundary.
4. **Implement** one coherent result, preserving unrelated work.
5. **Validate** with the smallest sufficient checks for the changed behavior and
   its concrete risks.
6. **Review the actual diff** against acceptance, compatibility, security, data
   and failure behavior.
7. **Correct** findings, revalidate and review the corrected content.
8. **Accept** an atomic result only when no blocking finding remains, then
   **reassess** the next dependent step from the new repository state.

Do not stop after planning when implementation and verification are possible.
Do not advance dependent work while a blocking finding is open.

## 6. Proof before implementation

Identify what proves a change before or alongside implementing it. For a defect,
reproduce the failure first. For new behavior, establish the fixture, contract
case or invariant that will fail when the behavior is wrong. Proof must fail
when the promised behavior breaks; a test that restates the implementation or
only inspects a success log is not proof.

| Change | Expected proof |
|---|---|
| Parser, serialization, state machine | Unit or contract tests with accepting and rejecting cases |
| Host import or capability | Actual component fixture: allowed, denied and failure paths |
| IPC or session protocol | Integration test over the real transport: framing, authorization, limits, cancellation |
| Session lifecycle | Child-process fixtures for each start, failure, cancellation and teardown path, checking owned resources settle |
| Rendering or presentation | Deterministic page with known pixels on the supported backend; readback after paint |
| Input or window management | Native input and geometry on a real compositor backend |
| Filesystem confinement | Traversal, symlink and replacement-race cases against the real implementation |
| Package trust, install or update | Tamper, interruption and data-retention fixtures |
| Installed system | Built and booted reference image |
| Dependency change | Resolved graph inspection plus the checks of every affected boundary |
| Performance claim | Repeatable measurement on the named target |
| Documentation or toolkit | Claims checked against source; links, catalog and metadata checks |

A change without an identifiable proof surface is not ready unless the
limitation is intrinsic and stated in the result. Never update expected output,
re-sign fixtures or regenerate artifacts merely to turn a failing check green;
explain the contractual reason for every changed expectation.

## 7. Definition of done

A change is done when its contract is coherent and evidenced, not when it
compiles.

- No synthetic success: feature-disabled builds, stubs and fallbacks must not
  report readiness, success or enforcement they do not provide.
- No silent no-ops for unsupported operations; fail explicitly with a useful
  diagnostic.
- No product UI or documentation that promises unavailable behavior.
- No placeholder paths left knowingly incomplete in a supported product path.
- Security and data preservation are part of the first implementation, not a
  later hardening stage.
- Accessibility, keyboard operation, scaling and declared keyboard layouts are
  product requirements where the change touches them.

Prefer small explicit APIs, deletion over new abstraction and existing concepts
over new ones. Add a dependency only for concrete value.

## 8. Maintenance

Maintenance work never rides inside an unrelated development branch and never
switches or edits the active development checkout.

An issue is justified only when all of these hold: a concrete affected user,
caller, contract, test harness or contributor can be named; repository evidence
demonstrates the problem; it matters in the current state; and it has an
independently verifiable acceptance criterion. Stylistic preferences,
hypothetical future needs and generic cleanup wishes do not qualify. Search for
an existing issue or pull request first and update it instead of duplicating.

One maintenance run handles one qualifying problem:

1. Verify the current default branch (`main`) by fetching it. If freshness
   cannot be verified, state the actual revision and the limitation.
2. Create one isolated worktree and one descriptive branch from it.
3. Follow the [reviewed loop](#5-reviewed-change-loop) to one reviewed pull
   request, or to a prepared local result when publication is not authorized.
4. Stop. Do not continue collecting opportunities.

The maintenance roles in [`.agents/agents/`](.agents/README.md#roles) apply this
section:

- **Architecture debt scout** inspects and reports the strongest justified
  issue. It edits no repository files and creates no implementation branches or
  commits. It may prepare or publish that issue only within granted authority.
  When nothing meets the threshold, it says so and stops.
- **Continuous simplicity** removes duplicated decisions, unnecessary state and
  misleading fallback paths without changing accepted behavior.
- **Continuous refactoring** changes internal structure while preserving accepted
  behavior, ownership and proof.

Neither simplification nor refactoring is a license for repeated architecture
replacement or line-count optimization at the expense of explicit authority,
errors or useful tests. If a maintenance change turns out to need a decision
under [section 9](#9-decisions-and-authority), stop it and record the decision
needed.

## 9. Decisions and authority

Make ordinary engineering decisions within accepted architecture without asking.
Decisions already accepted in the blueprint and authorization already granted
remain effective; do not request them again.

Seek a maintainer decision only for a material departure from accepted product
direction, public compatibility promises, trust boundaries, support scope or
destructive data policy, or when required authority is missing. Prepare the
problem, evidence, alternatives and trade-offs first, and continue independent
authorized work meanwhile. Do not disguise a change of direction as cleanup or
an implementation detail.

Publication, merge, release and deployment follow the authority actually
granted for them. A successful review, green CI or a passing hook grants none of
them by itself. Preserve branch protections, existing branches, other
contributors' work and unrelated working-tree changes.

## 10. Review

Review the actual change: a staged and unstaged diff, or an explicit
`git diff <base> <head>` range. An empty `git diff HEAD` after committing is not
a review of the committed change. Recheck material corrections, rebases and
conflict resolutions before integration, and review the final aggregate change
for cross-step effects before a pull request or release candidate.

Prefer a reviewer who did not author the change. When none is available, use a
fresh, separate critical pass and state that limitation honestly. Never present
self-review as independent review.

A **blocking** finding concerns correctness, promised behavior, trust, data
preservation, compatibility, accepted scope or required evidence. Fix it before
dependent work advances. An unrelated improvement goes to separately scoped
maintenance. Preferences are not blockers and findings are not a quota.

[Reviewer](.agents/agents/reviewer.md) describes the review procedure.

## 11. Validation

Run the smallest sufficient set for the change and expand it for a concrete
affected boundary, regression risk or release gate. Do not repeatedly run
unrelated expensive suites once the remaining risk is resolved, and never weaken,
skip or re-bless a required check to make it pass.

The [verification runner](.agents/scripts/verify.py) runs the repository's
actual check profiles; [verify](.agents/commands/verify.md) maps changes to
profiles and to evidence the runner cannot provide. [CI](.github/workflows/ci.yml)
remains the integration gate. Demo applications are separate workspaces; root
workspace tests do not cover them. Feature-disabled builds cannot stand in for
the `servo-embed` or `wasmtime-runtime` product paths.

## 12. Dependencies and forks

Servo is consumed from the `marcoallegretti/servo` fork (`servo-weft` branch).
Stylo currently resolves to upstream `servo/stylo`; the `marcoallegretti/stylo`
fork applies to WEFT only once a root-level override selects it and the resolved
graph confirms it. The selected revisions are those in `Cargo.lock`, not
`SERVO_PIN.md` or a branch name.

- Apply required Cargo overrides at the consuming workspace root. A dependency's
  own `[patch]` table does not apply transitively. Inspect the resolved graph and
  verify each claimed fix on the selected code.
- Keep a purpose, upstream status and removal condition for every retained fork
  patch.
- Local path overrides used for cross-repository development change the resolved
  graph. Do not commit the lockfile changes they cause and do not present
  results from them as evidence for the committed graph.
- A dependency update needs scope, compatibility evidence, relevant security
  review and the same reviewed loop as product code.
- Never fabricate `pkg-config` metadata, native library versions or readiness
  messages to make a build pass. Repair the supported environment or exclude
  the unsupported feature explicitly.
- Do not copy machine-specific usernames or checkout paths into documented
  workflows.

## 13. Versions and releases

Follow the blueprint's [version progression](BLUEPRINT.md#121-version-progression):
scoped `0.0.x` releases toward stable `0.1.0`, each moving through
`alpha.N -> beta.N -> rc.N -> final`, where final is the plain version without
a `-final` suffix. Assign the next `0.0.x` to an accepted
milestone scope after inspecting existing tags and published releases. Reconcile
the unreleased workspace value `0.1.0` deliberately.

Promotion is an explicit evidence-based decision under release authority. No
label, test count, absence of bug reports or successful build promotes a
candidate automatically. A release candidate requires the complete evidence for
its scope and an independent technical review of that exact candidate; an
unavailable required reviewer leaves the gate pending. Corrections after
candidate approval invalidate the affected evidence. Never rewrite or move
published tags or releases. Publish the artifact that was tested.

## 14. Branches, commits and publication

Branch from the verified current `main` unless the work depends on another
unmerged branch; then say so. Name every branch after the concrete change,
with a type prefix:

```text
fix/authorize-session-messages
refactor/share-servo-frame-loop
feat/keyboard-app-switching
build/enable-servo-vm-packages
test/reject-portal-symlink-escapes
docs/add-weft-contribution-contract
```

Reject task numbers, lane, phase or role labels, model or provider names and
generic names such as `maintenance`, `stability` or `cleanup`. Link the issue
from the pull request instead of naming the branch after it.

Commits are atomic, focused and revertible. Before committing, inspect
`git status` and the complete diff, and confirm no generated, transient or
local-environment files are included. Keep subjects short and technical; add a
body when it explains the problem, resulting behavior or evidence.

Branch names, commit messages, issues, pull requests, review comments and
release notes are written for maintainers: the concrete problem, the resulting
behavior, the affected boundaries and the validation actually performed. Do not
include model or provider attribution, co-author or generated-by trailers,
reviewer counts or personas, prompts, internal role handoffs or process
narration. A technical validation section with real commands and results is
useful.

Reusable governance files, the adopted blueprint and protocol or product
documentation are intentional tracked artifacts. Temporary plans, transcripts,
review notes, scratch output and internal progress reports stay out of Git.
A decision that exists only in a conversation does not exist for the project:
record durable decisions in the owning public artifact.

Before an action that can trigger automated public review or publication,
inspect the repository's integrations. Do not knowingly trigger publication that
conflicts with this section or bypass a required protection to avoid it.

## 15. Code

Follow the workspace configuration: edition 2024, the pinned toolchain in
[`rust-toolchain.toml`](rust-toolchain.toml) and `unsafe_code = "forbid"` where
crates inherit workspace lints. Keep `cargo fmt` and `clippy -D warnings`
clean. Do not silence a diagnostic without understanding its cause.

Product, helper and privileged subprocesses use exact executable paths and
argument arrays. Bound queues, frames, payloads and operation lifetimes. Keep
secrets, application payloads, clipboard contents and saved user data out of
default logs.

Prefer self-explanatory names and structure. Comment non-obvious invariants,
protocol constraints and safety reasoning; avoid narrative comments. Remove dead
code instead of commenting it out.
