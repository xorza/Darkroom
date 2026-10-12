# lumos-bench-finish: open questions

## Item 8, the stale quickbench line in `imaginarium/README.md`: where the commit goes

`imaginarium/README.md:15` reads "Benchmark harness lives in a sibling crate at `../quickbench/`
(referenced as a `dev-dependency` by relative path)." `imaginarium/Cargo.toml` has no such
dependency. The line is in the `imaginarium` submodule, a repository of its own, and the plan's run
may commit only to its darkroom branch.

| Option | What it does | Cost |
|---|---|---|
| **1. You commit it in `imaginarium` (recommended)** | Remove the line on `imaginarium`'s branch, push it, and bump the gitlink in darkroom. | Three steps by hand, in a repository this run may not touch. |
| 2. Leave it | The issue stays in `.notes/ISSUES.md`. | The README keeps naming a dependency that does not exist. |

Blocked: item 8 alone.
