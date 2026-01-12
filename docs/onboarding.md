# Scenarium Onboarding (Junior Dev)

Welcome! This is a short, practical guide to help you get productive fast.

## Project Overview

Scenarium is a Rust workspace for building node-based data processing pipelines, plus a visual editor for building and
executing graphs.

Repo layout:

- `common/` — shared utilities used across the workspace.
- `graph/` — the core graph library and execution logic.
- `editor/` — the visual editor application.
- `NOTES-AI.md` — current implementation notes and recent changes (kept up to date).

## Prerequisites

- Rust toolchain (edition 2024). Install via `rustup` if needed.
- Standard build tools for your OS (C/C++ compiler, etc.).

## Build and Run

From the repo root:

- Build everything:
  - `cargo build`
- Run the editor:
  - `cargo run`
- Run tests for the whole workspace:
  - `cargo test`

## Formatting and Linting

When you change Rust code, run this exact sequence:

```
cargo test && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
```

## Coding Guidelines (Project-Specific)

These are important for consistency and avoiding subtle bugs:

- Prefer crashing on logic errors over silently swallowing them.
- Use `Result<>` only for expected failures (I/O, network, user input, external services).
- Avoid `Option<>`/`Result<>` for cases that should not fail.
- For required values, use `.unwrap()`. For non-obvious cases, use `.expect("...")` with a clear message.
- Add `#[derive(Debug)]` to Rust structs.
- Add `assert!`s for function input/output invariants (not for user input or network failures).

## Where to Start in the Code

Suggested quick tour:

- `graph/` for graph data structures and execution logic.
- `editor/` for UI and editor behavior.
- `common/` for shared helpers and utility types.
