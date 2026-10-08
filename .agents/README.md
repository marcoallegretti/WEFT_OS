# WEFT OS contributor toolkit

Procedures, review roles, area knowledge and a verification runner for
contributors. [AGENTS.md](../AGENTS.md) is the engineering
contract; [BLUEPRINT.md](../BLUEPRINT.md) is the accepted product direction;
[CONTRIBUTING.md](../CONTRIBUTING.md) is the maintainer-facing summary.

These files help apply the contract. They do not grant permissions, approve
decisions, schedule work or change the [working mode](../AGENTS.md#4-working-modes).
If a guide here seems to disagree with `AGENTS.md`, the contract wins and the
guide needs fixing.

## Start

1. Read `AGENTS.md`, identify the accepted scope and the owning component.
2. Load only the skills that match the boundary you are changing.
3. Use [scope](commands/scope.md) for an unverified report or proposal, and
   [implement](commands/implement.md) for accepted, verifiable work.
4. Select checks and evidence with [verify](commands/verify.md), then have the
   [reviewer](agents/reviewer.md) inspect the actual diff.

## Commands

Commands are portable Markdown procedures. Follow them as checklists.

| Command | Input | Result |
|---|---|---|
| [scope](commands/scope.md) | Report, defect or proposal | Evidence-backed issue draft with owner, acceptance criterion and proof |
| [implement](commands/implement.md) | Issue or accepted scope | Reviewed atomic change on a descriptive branch |
| [verify](commands/verify.md) | Scope or diff | Coverage map and actual check results |
| [maintenance](commands/maintenance.md) | One named maintenance role | One justified issue, or one isolated reviewed change |
| [release](commands/release.md) | Milestone scope and candidate commit | Candidate evidence and a promotion recommendation |

## Roles

Roles are checklists for a reviewer or a focused pass over the code. A role
file does nothing until a task invokes it.

| Role | Responsibility |
|---|---|
| [reviewer](agents/reviewer.md) | Review a specified diff for correctness, scope, ownership and evidence without editing it |
| [architecture-debt-scout](agents/architecture-debt-scout.md) | Report the strongest present ownership or boundary problem; never edit repository files |
| [continuous-simplicity](agents/continuous-simplicity.md) | Remove one duplicated decision, unnecessary state or misleading fallback path |
| [continuous-refactoring](agents/continuous-refactoring.md) | Resolve one structural problem while preserving behavior, ownership and proof |

## Skills

Each skill is a `SKILL.md` with `name` and `description` metadata. The `weft-`
prefix keeps them distinct from skills installed by other projects.

| Skill | Load when changing |
|---|---|
| [weft-compositor-input](skills/weft-compositor-input/SKILL.md) | Smithay compositor, Wayland protocols, surfaces, focus, geometry, input, DRM or winit backends |
| [weft-servo-rendering](skills/weft-servo-rendering/SKILL.md) | Servo embedding, painting and presentation, the Servo fork and Stylo overrides, system UI and app pages |
| [weft-runtime-capabilities](skills/weft-runtime-capabilities/SKILL.md) | Wasmtime host, WIT, WASI preopens, host imports, grants, file portal, seccomp |
| [weft-appd-sessions](skills/weft-appd-sessions/SKILL.md) | Session lifecycle, readiness, child supervision, cleanup, appd IPC, WebSocket bridge |
| [weft-packages-data](skills/weft-packages-data/SKILL.md) | Manifests, `weft-pack`, signing and trust, installation, updates, app data, EROFS images |
| [weft-linux-deployment](skills/weft-linux-deployment/SKILL.md) | Nix flake and VM, systemd units, native dependencies, installed paths and endpoints |
| [weft-integration-verification](skills/weft-integration-verification/SKILL.md) | End-to-end fixtures, pixels and input, failure injection, reliability and evidence records |
| [weft-documentation](skills/weft-documentation/SKILL.md) | Public documentation, the blueprint, this toolkit, templates and the runner |

## Verification runner

[`scripts/verify.py`](scripts/verify.py) runs the repository's actual check
profiles. It needs Python 3.10 or newer. It works from any directory, runs fixed
argument arrays from the repository root without a shell, stops at the first
failure and returns its status. It never formats files, installs packages or
toolchain targets, regenerates expectations, signs packages, commits or
publishes.

```sh
python .agents/scripts/verify.py --help
python .agents/scripts/verify.py portable linux --dry-run
python .agents/scripts/verify.py portable linux
```

| Profile | Runs | Prerequisites |
|---|---|---|
| `toolkit` | Link, anchor, metadata, catalog and issue-form checks; runner tests | `pip install -r .agents/scripts/requirements.txt` |
| `portable` | `cargo fmt --check`; Clippy and tests for crates built on every host | Pinned toolchain |
| `linux` | Clippy and tests for `weft-compositor` and the feature-disabled shells | Linux Wayland, input, DRM and systemd development libraries |
| `servo-embed` | `cargo check` of both Servo hosts with the real renderer feature | As `linux`, plus the Servo build dependencies; long first build |
| `runtime` | Clippy and tests for `weft-runtime` with `wasmtime-runtime,net-fetch` | Pinned toolchain |
| `demos` | `cargo check` of the demo components for `wasm32-wasip2` | `rustup target add wasm32-wasip2` |
| `packages` | `weft-pack check` and `weft-pack verify` of the demo packages | Pinned toolchain |

All Cargo commands use `--locked`. Local `[patch]` overrides used for
cross-repository development change the resolved graph and make locked checks
fail; run the runner without them for evidence about the committed graph.

The runner covers compilation, lints, unit tests and package checks. It does not
run a compositor, render pixels, launch a session, boot the VM or exercise a
real component's host imports. Select that evidence with
[verify](commands/verify.md). Profiles are not a replacement for
[CI](../.github/workflows/ci.yml), and a passing profile is not approval to
publish, merge or release. No hooks are installed by this toolkit; if you call
the runner from a local hook, configure it in your own client or Git setup.

## Maintaining the toolkit

Put area knowledge in a skill, repeatable steps in a command, review criteria in
a role and executable checks in `scripts/`. Link to the contract and to the
owning source instead of restating policy or freezing implementation status.
List every command, role and skill in its table above; `toolkit` fails when a
document is not linked from this catalog. Changes to the runner need a test
showing failure behavior.

`check_toolkit.py` parses CommonMark links, images and reference links in this
directory and in the top-level contributor documents. It ignores code spans,
code blocks and HTML comments. It checks that local targets exist inside the
repository, that `#fragment` links into Markdown files match a GitHub-style
heading anchor, that metadata is a YAML mapping with unique string `name` and
`description` fields and a `name` matching its path, and that issue forms parse.
External URLs, raw HTML and the truth of the instructions still need review.

The structure follows the
[Resina Design System contributor toolkit](https://github.com/marcoallegretti/Resina-Design_System/tree/1e768064f57a1e403ab271822383e732795905a6/.agents)
at revision `1e768064f57a1e403ab271822383e732795905a6`. WEFT's contract,
ownership and evidence requirements determine its contents.
