# Project Agents.md Guide for OpenAI Codex and other AI agents

This Agents.md file provides comprehensive guidance for OpenAI Codex and other AI agents working with this codebase.

## Project Overview

 - Refer to [DOC.md](DOC.md) for high-level overview and common terms of the project.

## Coding Conventions

- Follow the existing code style in each file.
- Do not use Javascript. Use TypeScript instead.
- Add comments for complex logic
- Use meaningful variable and function names
- Update the documentation in the [DOC.md](DOC.md) file to reflect current state of the project.
  - Maintain project structure and organization.
  - Ensure that the documentation is clear and concise.
  - Add common terms and definitions to the documentation.
- Keep components generated small and focused
- Use asserts to check for invalid states in the code.
- Use asserts to validate inputs to functions.
- For rust projects:
  - Use cargo fmt to format the code and Clippy to lint the code.
  - Add tests for new features and bug fixes.
  - Use `cargo test --offline` to run the tests. `--offline` is required because Internet is disabled after the setup script runs.
  - Use `cargo clippy` to run the linter.
  - Use `cargo fmt` to format the code.
  - Add `Debug` derive to all structs and enums.
- For Svelte projects:
  - Use Svelte 5 runes syntax instead of Svelte 3 `$:` syntax.
  - Use Svelte 5's callback props instead of `createEventDispatcher` for event handling. `createEventDispatcher` is deprecated in Svelte 5.
  - Put shared TS types in [types.ts](ScenariumEditor.Svelte%2BTauri/frontend/src/lib/types.ts) and import them in the components.
    Do not duplicate types in multiple files.

