# lumos benches: finish the inventory

Scope: the work the bench inventory left, and the open issues in `.notes/ISSUES.md`. The bench
rules are the ones `lumos/src/bench/mod.rs` states.

## Items

Done in this order; each is deleted when it is committed.

8. **The `imaginarium` README names a quickbench dev-dependency that does not exist.**
   `imaginarium/README.md:15` says the benchmark harness lives in `../quickbench/` as a
   dev-dependency; `imaginarium/Cargo.toml` has none, and darkroom has no `quickbench` submodule
   any more. Remove the line. `imaginarium` is a submodule, a repository of its own: commit the
   edit there, and leave its push and the gitlink bump in darkroom to the user. Delete the issue
   from `.notes/ISSUES.md`.
