AI coding rules for this project:
- Avoid using Option<> and Result<> for cases that should not fail.
- For required values, use `.expect("...")` with a clear, specific message.
- Prefer crashing on logic errors rather than silently swallowing them.
- Use Result<> only for expected/legitimate failures (e.g., network, I/O, external services, user input).
- Always add `#[derive(Debug)]` to Rust structs.
- If Rust code was changed, run in following order:
  1. `cargo clippy --all-targets -- -D warnings`,
  2. `cargo test`
  3. `cargo check`, 
  4. `cargo fmt`
  before confirming output.
- Add asserts for function input arguments and outputs where applicable, so logic errors crash instead of being swallowed. Do not use asserts for user input and possible network failures.
- Check online documentation for best practices and patterns.
- Update README.md with any changes to the project.
- `NOTES-AI.md` is AI-generated and contains implementation details, project structure, and functionality notes. Avoid adding implementation specifics to `README.md`; instead, update `NOTES-AI.md` and keep it current for fast AI access. Do not store changes there, only current state of the project.
