---
name: weft-documentation
description: Use for WEFT OS public documentation, the blueprint, this contributor toolkit, issue and pull request templates and the verification runner.
---

# Documentation and the toolkit

[AGENTS.md](../../../AGENTS.md) is the only policy source. The
[blueprint](../../../BLUEPRINT.md) holds accepted direction. Public documentation
in [README.md](../../../README.md) and [docs/](../../../docs/architecture.md)
describes implemented behavior and must agree with source.

## Working rules

- Verify every command, path, feature, environment variable and API you document
  against the current source, and run commands where practical. Keep blueprint
  intent, implemented behavior and executed evidence distinct: "implemented",
  "verified" and "planned" are different claims.
- Correct documentation that contradicts source instead of working around it;
  for example, a pin file that disagrees with `Cargo.lock`, or a signing
  description that differs from the implemented inventory.
- Link durable decisions and requirements to their owning artifact. Toolkit files
  provide procedures and area knowledge, never a second policy, permissions,
  schedules or an internal log.
- Write issues, pull requests, commits and release notes for maintainers: the
  problem, the resulting behavior and the evidence. No attribution, trailers,
  prompts or process narration.
- Keep Markdown portable: standard CommonMark links and tables, YAML front matter
  with `name` and `description` only, no client-specific syntax required to
  follow a procedure.

## Toolkit changes

Add area knowledge in a skill, repeatable steps in a command, review criteria in
a role and executable checks in `.agents/scripts/`. Update the
[catalog](../../README.md) for every new document. Then run:

```sh
pip install -r .agents/scripts/requirements.txt
python .agents/scripts/verify.py toolkit
```

That checks links, anchors, metadata, catalog coverage and issue forms, and runs
the runner tests. For runner changes, add tests for failure propagation,
unknown input, missing executables and execution from another directory. Review
the instructions manually against `AGENTS.md`, the blueprint and CI; passing
checks does not establish that the instructions are true.

Changes to the blueprint or contract are governance changes: follow the
[reviewed loop](../../../AGENTS.md#5-reviewed-change-loop), check every document
that links to the changed sections, and seek a
[decision](../../../AGENTS.md#9-decisions-and-authority) for a material change of
direction.
