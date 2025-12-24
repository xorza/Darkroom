# Scenarium Implementation Notes (AI)

This file captures implementation details and internal structure for AI agents.

## Project Overview

Scenarium collects the tooling required to build node based applications. The repository is a Rust workspace containing the core graph implementation and an editor front end.

## Repository Layout

- **common** – shared utilities and helper macros
- **graph** – Rust library defining graphs, nodes and the runtime
- **test_resources** – sample graphs, function libraries and media used by tests
- **test_output** – output folder created by unit tests
- **deprecated_code** – older .NET and QML editors and experimental code

Each directory may contain its own `Cargo.toml` or build scripts.
The root `Cargo.toml` defines the workspace and shared dependencies.

Commit messages are often prompts sent to an AI agent to request a change.

## Dependency Versions

As of 2025-12-24, workspace dependencies are set to the latest minor versions with patch versions left open.
The only version bump needed in this update was `wgpu` to `28.0`; other workspace dependencies were already at their latest minor versions.

## Subprojects

### common crate

Provides small utilities such as logging helpers, macros for unique identifier types, and miscellaneous functions.
See `common/src` for implementation details.

Recent adjustments:
- `common/src/scoped_ref.rs` uses generic drop callbacks (no boxing), derives `Debug` on scoped refs, and uses `expect` in `Drop` for invariant enforcement.
- Removed `common/src/apply.rs` and replaced its usages with standard `Option` methods.

### graph crate

Implements the data structures for graphs and nodes. Nodes are created from functions defined in a function library.
Connections between nodes are represented by `Binding::Output` values.
Data structures like graph and function library can be serialized to YAML files.

Runtime execution is handled by the `runtime_graph` module which determines which nodes should run each tick.
Additional modules drive execution and integration:
- `compute` runs active nodes through an `Invoker` and converts values between data types.
- `invoke` defines the `Invoker` trait and the `UberInvoker` aggregator which dispatches function calls.
- `worker` spawns a Tokio thread that executes the graph either once or in a loop and processes events.
- `event` manages asynchronous event loops that send event IDs back to the worker.

## Common Terms

Add shared terminology here as the project evolves.

## Editor Workflow

Add editor workflow details here as they are defined.
