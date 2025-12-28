# Scenarium

Scenarium is a collection of tools and libraries for building node based data processing pipelines.
The core graph and utilities are written in Rust.

## Repository Layout

- **common** – shared utilities used across the workspace
- **graph** – the main graph library
- **deprecated_code** – older editors and experimental code
- **test_resources** – sample assets used by tests

## Maintenance

Workspace dependencies are kept at their latest minor versions (patch versions left open).
Common utilities receive periodic safety and ergonomics updates.
Shared utilities favor standard library functionality over custom helpers.
Logging uses the tracing ecosystem.
Runtime validation catches invalid bindings earlier.
Some tests run on the Tokio async runtime.
Option-based lookups are preferred with explicit `expect` at call sites.
Synchronization uses Tokio primitives; parking_lot is not used.
Async contexts use Tokio locks with awaits where possible.
Blocking helpers are removed in favor of async/await or non-blocking try_lock.
Core invocation and compute paths are async.
Runtime graph traversal is designed to handle large graphs safely.
Runtime graph construction separates node collection, dependency propagation, and scheduling.
Runtime graph propagation asserts nodes were processed before computing input state.
Runtime graph propagation assumes function library alignment and asserts on missing functions.
Runtime graph builds assume validated graphs and function libraries.
Runtime node collection centralizes reuse of prior runtime state.
Runtime graph scheduling asserts when runtime nodes are missing.
Runtime graph propagation asserts when output bindings point at missing runtime nodes.
Runtime graph propagation panics with function + node IDs when functions are missing.
Runtime graph assertions include node indices and IDs for easier debugging.
Runtime graph assertions include mismatch context to speed up diagnosis.
Worker event loops terminate when their message channel closes.
Runtime node updates use shared reset logic to keep state consistent.
Runtime nodes own their own reset behavior for update passes.
Runtime node port resets reuse a helper to keep input/output sizing consistent.
Runtime graph updates handle dynamic graph changes across runs.
Compute execution follows the runtime scheduling order.
Compute input setup now uses a straight loop to avoid layered iterator chains.
Runtime graph invariant lookups use explicit `expect` messages for clarity.
Runtime graph traversal comments now document the update/backward/forward phases.
Runtime node cache rebuilds drop runtime nodes that are no longer in the graph.
Editor debug tasks can be configured per-project.
Runtime node cache compacts runtime nodes in-place to avoid extra allocations.
Runtime node cache compaction now has inline comments explaining the swap/truncate flow.
Internal refactors are documented in `NOTES-AI.md` to keep this README high-level.
Tests use the in-code `test_graph()` fixture for the standard sample graph.

## Benchmarks

Benchmarks live under `graph/benches` and use Criterion.
Run `cargo bench -p graph --bench b1` to execute the current benchmark.

## License

This project is licensed under the terms of the MIT license.
See [LICENSE](LICENSE) for more information.
