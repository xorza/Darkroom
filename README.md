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
Worker event loops terminate when their message channel closes.
Runtime node updates use shared reset logic to keep state consistent.
Runtime nodes own their own reset behavior for update passes.
Runtime graph updates handle dynamic graph changes across runs.
Compute execution follows the runtime scheduling order.
Editor debug tasks can be configured per-project.

## Benchmarks

Benchmarks live under `graph/benches` and use Criterion.
Run `cargo bench -p graph --bench b1` to execute the current benchmark.

## License

This project is licensed under the terms of the MIT license.
See [LICENSE](LICENSE) for more information.
