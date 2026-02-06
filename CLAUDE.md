AI coding rules for Rust projects:

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
- Add tests for all new or modified non-GUI code (algorithms, data structures, utilities).
- Check test run times are reasonable. Research and fix slow tests.
- Check online documentation for best practices and patterns.

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
