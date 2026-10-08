# Contributing to WEFT OS

[AGENTS.md](AGENTS.md) is the engineering contract for every contribution,
manual or assisted. [BLUEPRINT.md](BLUEPRINT.md) describes the accepted product
direction toward `0.1.0`. This guide points to the workflow; the contract
defines the requirements.

The [contributor toolkit](.agents/README.md) provides procedures, review roles,
area knowledge and a verification runner. The same files work for manual
development and any coding assistant; its setup table covers client
differences.

## Describe a verifiable change

Search [existing issues](https://github.com/marcoallegretti/WEFT_OS/issues) and
pull requests first, then open a
[new issue](https://github.com/marcoallegretti/WEFT_OS/issues/new/choose):

- **Defect or change** for a concrete problem or ordinary work within accepted
  architecture.
- **Maintenance** for one present duplication, structural or misleading-fallback
  problem outside active development.
- **Decision** for a material change of product direction, public compatibility,
  trust boundaries, support scope or data policy. Opening it does not approve the
  change.

Each form asks for observed and expected behavior, evidence, the owning
component, present impact, an acceptance criterion and the proof that would
show the change works. The [maintenance threshold](AGENTS.md#8-maintenance)
separates a present problem from a cleanup preference.

## Find the owner

Each component owns specific responsibilities, listed in the blueprint's
[ownership table](BLUEPRINT.md#4-architecture-and-ownership). Public contracts
are the package manifests, the [host WIT](crates/weft-runtime/wit/weft-app.wit),
the [shell protocol](protocol/weft-shell-unstable-v1.xml) and the
[IPC types](crates/weft-ipc-types/src/lib.rs). Selected dependency APIs are
those in [Cargo.lock](Cargo.lock), including the Servo fork and the Stylo revision.

## Prove and implement the change

Follow the [reviewed change loop](AGENTS.md#5-reviewed-change-loop): confirm
scope, name the proof, implement, validate, review the actual diff, correct and
commit atomically. Reproduce a defect before fixing it. Compilation, mocks and
capability declarations do not prove rendering, native input, isolation or
persistence; see [proof before implementation](AGENTS.md#6-proof-before-implementation).

Branch from the current `main` with a descriptive name such as
`fix/authorize-session-messages` or `refactor/share-servo-frame-loop`; the
[branch rules](AGENTS.md#14-branches-commits-and-publication) list what to avoid.

Install the native dependencies from [docs/building.md](docs/building.md) and
run the relevant [runner](.agents/README.md#verification-runner) profiles, for
example:

```sh
python .agents/scripts/verify.py portable linux
python .agents/scripts/verify.py servo-embed
pip install -r .agents/scripts/requirements.txt
python .agents/scripts/verify.py toolkit
```

[verify](.agents/commands/verify.md) maps each kind of change to its profiles and
to the evidence the runner cannot provide, such as a running compositor, real
pixels, actual components or a booted image. Report checks that could not run
and why.

## Submit a pull request

The [pull request template](.github/pull_request_template.md) asks for the
problem and scope, what changed, ownership, what proves it, the validation
actually performed, manual verification and material limitations. Write for a
maintainer who has no access to your working notes: no attribution, generated-by
or co-author trailers, prompts or process narration.

Contribution work is reviewed from its actual diff, preferably by someone who
did not write it. Maintainers decide merges and release promotion; a green CI
run or a review does not merge or release anything by itself.
