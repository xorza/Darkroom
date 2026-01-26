AI coding rules for Rust projects:

- Avoid using Option<> and Result<> for cases that should not fail.
- For required values, use `.unwrap()`. For non-obvious cases add `.expect("...")` with clear, specific message.
- Prefer crashing on logic errors rather than silently swallowing them.
- Use Result<> only for expected/legitimate failures (e.g., network, I/O, external services, user input).
- Always add `#[derive(Debug)]` to Rust structs.
- If Rust code was changed, run in following order:
    1. `cargo test && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings`
       before confirming output. Skip doc-tests.
- Add asserts for function input arguments and outputs where applicable, so logic errors crash instead of being
  swallowed. Do not use asserts for user input and possible network failures.
- Check online documentation for best practices and patterns.
- Update README.md with any changes to the project.
- `NOTES-AI.md` is AI-generated and contains implementation details, project structure, and functionality notes. Avoid
  editing `README.md` unless asked; instead, update `NOTES-AI.md` and keep it current for fast AI access. Do not store
  changes there, only current state of the project.
- Read `NOTES-AI.md` for summarized knowledge about the project.
- When running benchmarks, use `cargo bench -p <crate> --features bench --bench <name>` to compile only the needed
  crate and enable the bench feature (e.g., `cargo bench -p lumos --features bench --bench math`).
- Use nightly -Z unstable-options --report-time for measuring test execution time when asked
