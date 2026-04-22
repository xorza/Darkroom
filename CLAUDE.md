AI coding rules for Rust projects:

## Workflow

- **Never commit or push without explicit user confirmation.** This rule is non-negotiable and overrides auto mode, "just do it" instructions, or any implied approval from earlier in the conversation. The trigger must be a fresh, unambiguous command like "commit", "commit push", "ship it". "Do the refactor" / "apply F3" / "go" authorize the code change, not the commit. Finish the change, run tests/clippy/fmt, then stop and wait for the user to inspect the diff and explicitly say to commit.

## Available CLI tools

These are installed and available — prefer them over slower/regex-based equivalents:

- **`ast-grep`** — AST-aware structural code search. Use for "find all call sites of X", "find all `Arc::get_mut().unwrap()` patterns", or any search where regex is fragile. Beats `rg` for code-pattern matching.
  - `ast-grep run --pattern 'Arc::get_mut($X).unwrap()' --lang rust`
- **`scc`** — fast LOC counter with language stats. Use for design-review scope decisions (faster + smarter than `wc -l`).
  - `scc prism/src/gui/`
- **`hyperfine`** — statistical benchmarking. Use when validating performance claims rather than guessing.
  - `hyperfine 'cargo test --release my_test'`
- **`cargo-machete`** — finds unused crate dependencies.
  - `cargo machete`
- **`bacon`** — Rust-aware continuous build/test loop. Useful when iterating on a refactor.
  - `bacon` (in repo root)

Plus the standard set: `rg`, `fd`, `jq`, `gh`, `cargo`, `cargo-nextest`, `rustfmt`, `clippy`, `sqlite3`.

## Error Handling

- Use `Result<>` only for expected failures (network, I/O, external services, user input).
- Avoid `Option<>` and `Result<>` for cases that cannot fail.
- For required values, use `.unwrap()`. For non-obvious cases, use `.expect("clear message")`.
- Crash on logic errors. Do not silently swallow them.
- Add asserts for function inputs and outputs to catch logic errors. Do not assert on user input or network failures.

## Code Style

- Always add `#[derive(Debug)]` to structs.
- No backward compatibility. Remove old/deprecated code, rename freely, change APIs. Rewrite callers to use new APIs. No compatibility shims, re-exports, or wrappers.
- Remove unused code. If kept intentionally, add a comment explaining why and silence linter warnings.
- Keep public API clean and consistent.
- Never use `#[cfg(test)]` on functions in production code. If tests need convenience helpers, define them in the test module itself.

## Verification

- After changing code, run before confirming:
  ```
  cargo nextest run && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
  ```
  Skip doc-tests.
- Check test run times are reasonable. Research and fix slow tests.
- Check online documentation for best practices and patterns.

## Testing

- Write tests for ALL new and modified non-GUI code. No exceptions.
- Tests must verify **correctness**, not just "it runs without panicking":
  - Use hand-computed expected values. Show the math in comments.
  - Assert exact outputs (survivor counts, indices, computed values), not vague ranges.
  - Verify edge cases: empty input, minimal input, boundary conditions.
- For algorithms with parameters, test that parameters actually change behavior:
  - Test with parameter A: expect result X. Test with parameter B: expect result Y. Assert X != Y.
- For SIMD implementations, test against scalar reference for identical results.
- For rejection/filtering: verify exactly which elements survive and which are rejected.
- For numerical code: validate against known-good reference values or analytical solutions.
- Do NOT write tests that only check `result < 10` or `remaining > 0`. These catch nothing.

## UI conventions (egui, prism crate)

- **Every widget id must come from `StableId`** (`prism/src/common/id_salt.rs`).
  Sanctioned constructors:
  - `StableId::new(name)` — `#[track_caller]` mixes `file!()`/`line!()` into
    the hash. Use a tuple for per-instance widgets:
    `StableId::new(("cache_btn", node.id))`.
  - `StableId::from_id(id)` — wrap an existing `egui::Id` (e.g. inherited
    from a caller). No rehash.
- **Never call `UiBuilder::new()` directly.** Use `gui.scope(id).show(|gui| ...)`
  (optionally chain `.max_rect(r)` / `.sense(s)` before `.show`).
  It applies `UiBuilder::id(id.id())` (`global_scope=true`) so the scope's
  registered widget id equals the salt verbatim — bypassing egui's
  `unique_id = stable_id.with(parent_counter)` formula
  (`egui-0.34.1/src/ui.rs:297`) that drifts whenever conditional siblings
  appear/disappear in the parent and trips the
  "widget rect changed id between passes" warning.
- **Never call bare `ui.allocate_rect`/`allocate_exact_size`/`allocate_space`
  on a `Gui<'_>`'s parent ui.** Wrap in a `gui.scope(StableId::new(..)).show(...)`
  first so the auto-id starts from a stable seed.
- **Don't bake transient runtime keys (e.g. `selected_node_id`) into a
  fixed-rect widget's salt.** The widget id changes on every selection
  while its rect stays constant → "rect changed id" warning. Use a stable
  string salt; let the *content* change instead.
- **Whitelisting**: if you genuinely need raw `UiBuilder::new(` (e.g.
  inside a function that takes raw `egui::Ui`, not our `Gui`), put
  `// id-drift-ok` on the same line OR up to two lines above. The
  tripwire test `no_bare_ui_builder_in_crate` enforces all of this.

## Documentation

- Read `NOTES-AI.md` files for summarized project knowledge. Check current directory and relevant subdirectories.
- `NOTES-AI.md` files are AI-generated notes on implementation details and structure. Place in any directory where context is needed. Store only current state, not change history. Split files >300 lines into subdirectory files with a brief parent overview.
- Avoid editing root `README.md` unless asked; update `NOTES-AI.md` instead.
- Add `README.md` to folders that benefit from human-readable docs (crates, examples, benchmarks, complex modules).

## Optimization Workflow

- Before optimizing, always run or create a relevant benchmark and save the baseline results.
- After optimizing, run the same benchmark again and compare against the baseline to verify the optimization actually improved performance.
- If the optimization is a regression or no improvement, revert it.

## Benchmarks and Profiling

- Run benchmarks: `cargo test -p <crate> --release <bench_name> -- --ignored --nocapture`
- Save benchmark results to a txt file in the bench directory. Maintain a `bench-analysis.md` with interpretations. Update on re-runs.
- Add readme files to benchmark folders explaining which optimizations were tried.
- Use nextest for running tests and measuring execution time.
- Perf profiling: use 3000 samples per second.
- If `addr2line` errors appear in `perf report`/`perf script`, use `perf script --no-inline`.
