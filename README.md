# Scenarium

Scenarium is a collection of tools and libraries for building node based data processing pipelines.
The core graph and utilities are written in Rust.

## Repository Layout

- **common** – shared utilities used across the workspace
- **graph** – the main graph library
- **deprecated_code** – older editors and experimental code
- **test_resources** – sample graphs and media used by tests

## Maintenance

Workspace dependencies are kept at their latest minor versions (patch versions left open).
Workspace crates should declare shared dependencies via `workspace = true`.
Common utilities receive periodic safety and ergonomics updates.
Shared utilities favor standard library functionality over custom helpers.
Logging uses the tracing ecosystem.
Execution validation catches invalid bindings earlier.
Some tests run on the Tokio async execution.
Option-based lookups are preferred with explicit `expect` at call sites.
The graph crate exposes a public prelude for common types, including execution graph helpers.
Workspace crates should prefer importing shared graph types via that prelude.
The editor can build its view graph from core graph data.
Editor graph view types live under `editor/src/model/` in `graph_view.rs` and `node_view.rs`.
Editor graph serialization now uses `common::FileFormat` (JSON/YAML).
The editor loads the core test function library to label node inputs/outputs.
Editor view tests now build from the core test graph and test function library.
Execution graph tests now cover pure vs impure function execution and Output behavior checks.
Synchronization uses Tokio primitives; parking_lot is not used.
Async contexts use Tokio locks with awaits where possible.
Blocking helpers are removed in favor of async/await or non-blocking try_lock.
Core invocation and compute paths are async.
Execution graph traversal is designed to handle large graphs safely.
Execution graph construction separates node collection, dependency propagation, and scheduling.
Execution graph propagation asserts nodes were processed before computing input state.
Execution graph propagation assumes function library alignment and asserts on missing functions.
Execution graph builds assume validated graphs and function libraries.
Execution node collection centralizes reuse of prior execution state.
Execution graph scheduling asserts when execution nodes are missing.
Execution graph propagation asserts when output bindings point at missing execution nodes.
Execution graph propagation panics with function + node IDs when functions are missing.
Execution graph assertions include node indices and IDs for easier debugging.
Execution graph assertions include mismatch context to speed up diagnosis.
Worker event loops terminate when their message channel closes.
Execution node updates use shared reset logic to keep state consistent.
Execution nodes own their own reset behavior for update passes.
Execution node port resets reuse a helper to keep input/output sizing consistent.
Execution graph updates handle dynamic graph changes across runs.
Compute execution follows the execution scheduling order.
Compute input setup now uses a straight loop to avoid layered iterator chains.
Execution graph invariant lookups use explicit `expect` messages for clarity.
Execution graph traversal comments now document the update/backward/forward phases.
Execution node cache rebuilds drop execution nodes that are no longer in the graph.
Editor debug tasks can be configured per-project.
Execution node cache compacts execution nodes in-place to avoid extra allocations.
Execution node cache compaction now has inline comments explaining the swap/truncate flow.
Internal refactors are documented in `NOTES-AI.md` to keep this README high-level.
Editor UI behavior tweaks are documented in `NOTES-AI.md`.
Editor render ordering adjustments are documented in `NOTES-AI.md`.
Editor node rendering consolidation is documented in `NOTES-AI.md`.
Editor node ordering behavior is documented in `NOTES-AI.md`.
Editor port hit-testing ordering is documented in `NOTES-AI.md`.
Editor connection drag rendering is documented in `NOTES-AI.md`.
Lua file format support is documented in `NOTES-AI.md`.
Common file format serialization helpers are documented in `NOTES-AI.md`.
Common serialization error types are documented in `NOTES-AI.md`.
Editor run controls are documented in `NOTES-AI.md`.
Editor compute status hooks are documented in `NOTES-AI.md`.
GraphView to Graph conversion is documented in `NOTES-AI.md`.
Lua integration updates are tracked in `NOTES-AI.md`.
Function lambda storage refinements are tracked in `NOTES-AI.md`.
Lua output formatting updates are tracked in `NOTES-AI.md`.
Lua function loading refactors are tracked in `NOTES-AI.md`.
Lua value conversion refinements are tracked in `NOTES-AI.md`.
Lua function parsing refinements are tracked in `NOTES-AI.md`.
Lua graph connection extraction tweaks are tracked in `NOTES-AI.md`.
Lua graph wiring refinements are tracked in `NOTES-AI.md`.
Lua test cleanup notes are tracked in `NOTES-AI.md`.
Lua test coverage updates are tracked in `NOTES-AI.md`.
Compute error serialization notes are tracked in `NOTES-AI.md`.
Compute error source storage updates are tracked in `NOTES-AI.md`.
Compute invocation error handling updates are tracked in `NOTES-AI.md`.
Execution graph traversal cleanup notes are tracked in `NOTES-AI.md`.
Execution graph validation invariants are tracked in `NOTES-AI.md`.
Tests use the in-code `test_graph()` fixture for the standard sample graph.
Tests use in-code function library fixtures for the standard sample functions.
Graph and function library file loaders now auto-detect YAML or JSON based on file extension.
Function libraries now own invocation lambdas instead of a separate invoker layer.

## Benchmarks

Benchmarks live under `graph/benches` and use Criterion.
Run `cargo bench -p graph --bench b1` to execute the current benchmark.

## License

This project is licensed under the terms of the MIT license.
See [LICENSE](LICENSE) for more information.
