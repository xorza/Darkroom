# Project Agents.md Guide for OpenAI Codex and other AI agents

This Agents.md file provides comprehensive guidance for OpenAI Codex and other AI agents working with this codebase.

## Project Overview

 - Refer to [DOC.md](DOC.md) for high-level overview and common terms of the project.

## Coding Conventions

- Use Svelte 5 runes syntax instead of Svelte 3 `$:` syntax.
- Use Svelte 5's callback props instead of `createEventDispatcher` for event handling. `createEventDispatcher` is deprecated in Svelte 5.
- OpenAI Codex should follow the existing code style in each file.
- Do not use Javascript. Use TypeScript instead.
- Add comments for complex logic
- Use meaningful variable and function names
- Update the documentation in the [DOC.md](DOC.md) file to reflect current state of the project.
  - Maintain project structure and organization.
  - Ensure that the documentation is clear and concise.
  - Add common terms and definitions to the documentation.
- Keep components generated small and focused
- For rust projects:
  - Use cargo fmt to format the code and Clippy to lint the code.
  - Add tests for new features and bug fixes.

