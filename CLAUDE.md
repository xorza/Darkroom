AI coding rules for Rust projects:

- Avoid `Option<>` and `Result<>` for cases that cannot fail.
- For required values, use `.unwrap()`. For non-obvious cases, add `.expect("...")` with a clear, specific message.
- Prefer crashing on logic errors rather than silently swallowing them.
- Use `Result<>` only for expected failures (e.g., network, I/O, external services, user input).
- Always add `#[derive(Debug)]` to Rust structs.
- After changing Rust code, run in this order before confirming output:
    1. `cargo nextest run && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings`
    2. Skip doc-tests.
- Add asserts for function inputs and outputs where applicable so logic errors crash instead of being swallowed. Do not use asserts for user input or network failures.
- Check online documentation for best practices and patterns.
- `NOTES-AI.md` files are AI-generated and contain implementation details, structure, and functionality notes. They can be placed in any directory where context is needed (root, crates, modules, etc.). Avoid editing root `README.md` unless asked; instead, update the relevant `NOTES-AI.md` and keep it current. Store only current state, not change history.
- Read `NOTES-AI.md` files for summarized project knowledge. Check for them in the current working directory and relevant subdirectories.
- When a `NOTES-AI.md` file becomes too large (>300 lines or covers multiple distinct modules), split it into smaller files in corresponding subdirectories. Keep the parent file as a brief overview with references to child files.
- Add `README.md` files to any folder that benefits from human-readable documentation (e.g., crates, examples, benchmarks, complex modules).
- When running benchmarks, use `cargo test -p <crate> --release <bench_name> -- --ignored --nocapture` to run benchmark tests (e.g., `cargo test -p lumos --release bench_extract_candidates -- --ignored --nocapture`).
- When running perf profiling use 3000 samples per second
- Use nextest for running tests and for measuring test execution time when asked.
- For iterative changes and benchmarks, add readme files to corresponding folders explaining which optimizations were implemented and which were removed. When running benchmarks, always output results to a txt file in the bench directory and maintain a bench-analysis.md file with interpretations. Update it when re-running benchmarks.
- Remove deprecated. Make sure public api is properly exposed and consistent. If code is unused but expected to be used later, add a comment explaining why it is kept and silence linter warnings if needed.
- Check test run time make sure tests taking reasonable amount of time. If tests run too long, research and improve.
