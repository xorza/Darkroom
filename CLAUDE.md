AI coding rules for the Imaginarium project (single-crate image processing / pipeline library).

General Rust coding conventions live in [`CODING_STYLE.md`](CODING_STYLE.md) — comments, asserts, visibility, trivial accessors, tuple returns, inline paths, re-exports, test layout, and mechanical-refactoring tooling. Read it first. This file holds only Imaginarium-specific rules and project workflow.

## Workflow

- **Never commit or push without explicit user confirmation.** This rule is non-negotiable and overrides auto mode, "just do it" instructions, or any implied approval from earlier in the conversation. The trigger must be a fresh, unambiguous command like "commit", "commit push", "ship it". "Do the refactor" / "apply F3" / "go" authorize the code change, not the commit. Finish the change, run tests/clippy/fmt, then stop and wait for the user to inspect the diff and explicitly say to commit.
- **Use `.tmp/` for source investigation of external dependencies.** Cloning a dep into `.tmp/<crate>` (gitignored) lets Read/Grep work without per-file approval prompts on cargo registry paths. Match the version to the one resolved in `Cargo.lock`. Leave clones in place across sessions — `.tmp/` is gitignored and persists.

## Layout

Single-crate project — root `Cargo.toml` is the package manifest.

- `src/` — library source.
- `examples/` — runnable examples.
- `test_resources/`, `test_output/` — fixtures and writeable test outputs.
- `../quickbench/` — sibling crate (benchmark harness + `quickbench-macros` proc-macro). Referenced as a `dev-dependency` by relative path; not part of this repo.

## Available CLI tools

These are installed and available — prefer them over slower/regex-based equivalents:

- **`ast-grep`** — AST-aware structural code search. Use for "find all call sites of X", "find all `Arc::get_mut().unwrap()` patterns", or any search where regex is fragile. Beats `rg` for code-pattern matching.
  - `ast-grep run --pattern 'Arc::get_mut($X).unwrap()' --lang rust`
- **`scc`** — fast LOC counter with language stats. Use for design-review scope decisions (faster + smarter than `wc -l`).
  - `scc src/`
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

See `CODING_STYLE.md` for the general rules. Imaginarium-specific additions:

- Always add `#[derive(Debug)]` to structs.
- No backward compatibility. Remove old/deprecated code, rename freely, change APIs. Rewrite callers to use new APIs. No compatibility shims, re-exports, or wrappers.
- Remove unused code. If kept intentionally, add a comment explaining why and silence linter warnings.
- Keep public API clean and consistent.
- **No decorative section-divider comments.** Do not add `// =====…`, `// -----…`, or standalone `// Title` lines that exist only to group items inside a file. If a file has grown big enough to feel like it needs sections, split it into submodules instead.

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

- Run benchmarks: `cargo test --release <bench_name> -- --ignored --nocapture`
- Save benchmark results to a txt file in the bench directory. Maintain a `bench-analysis.md` with interpretations. Update on re-runs.
- Add readme files to benchmark folders explaining which optimizations were tried.
- Use nextest for running tests and measuring execution time.
- Perf profiling: use 3000 samples per second.
- If `addr2line` errors appear in `perf report`/`perf script`, use `perf script --no-inline`.
