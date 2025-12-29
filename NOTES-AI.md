# Scenarium Implementation Notes (AI)

This file captures implementation details and internal structure for AI agents.

## Project Overview

Scenarium collects the tooling required to build node based applications. The repository is a Rust workspace containing the core graph implementation and an editor front end.

## Repository Layout

- **common** – shared utilities and helper macros
- **graph** – Rust library defining graphs, nodes and the execution
- **test_resources** – sample graphs and media used by tests
- **test_output** – output folder created by unit tests
- **deprecated_code** – older .NET and QML editors and experimental code

Each directory may contain its own `Cargo.toml` or build scripts.
The root `Cargo.toml` defines the workspace and shared dependencies.
The graph crate exports a `prelude` module that re-exports common graph, data, function, and execution graph types for easier imports.
The editor now imports `Graph` via `graph::prelude::Graph`.
The editor model exposes `Graph::from_graph` to convert a core graph into the editor view with inferred output counts and a simple grid layout.
Editor view model types are split into `editor/src/model/graph_view.rs` and `editor/src/model/node_view.rs`.
Editor graph serialization now uses `common::FileFormat` (JSON/YAML).
The editor loads `test_func_lib` to name view-node inputs/outputs based on core function definitions.
Editor view tests now derive their graph view from `test_graph` + `test_func_lib` instead of a standalone view fixture.
Execution graph tests now verify that impure functions execute without input changes, pure functions do not, and Output behaviors panic unless nodes are Terminal.

Commit messages are often prompts sent to an AI agent to request a change.

## Dependency Versions

As of 2025-12-24, workspace dependencies are set to the latest minor versions with patch versions left open.
The only version bump needed in this update was `wgpu` to `28.0`; other workspace dependencies were already at their latest minor versions.
The editor crate now relies on `workspace = true` for shared dependencies, and the workspace dependency list includes `toml`, `rayon`, `dotenv`, and `tracing-rolling-file`.

## Subprojects

### common crate

Provides small utilities such as logging helpers, macros for unique identifier types, and miscellaneous functions.
See `common/src` for implementation details.

Recent adjustments:
- `common/src/scoped_ref.rs` uses generic drop callbacks (no boxing), derives `Debug` on scoped refs, and uses `expect` in `Drop` for invariant enforcement.
- Removed `common/src/apply.rs` and replaced its usages with standard `Option` methods.
- Switched logging to tracing; `common/src/log_setup.rs` wires console + rolling file output via `tracing-subscriber` and `tracing-appender`.
- Execution graph construction now uses a DFS order with cycle detection, input/output binding validation, and ID-to-index maps for faster lookups.
- Added `*_ref` accessors on `Graph`/`FuncLib`/`ExecutionGraph` for invariant-driven access (panic on missing IDs).
- Replaced many unwraps with explicit `expect` messages in core execution paths.
- Invoke tests now use Tokio async tests with blocking sections around sync compute execution.
- Removed `*_ref` accessors and standardized on `Option` lookups with explicit `expect` messages at call sites.
- Replaced parking_lot usage with Tokio mutexes and async/try_lock access patterns (no blocking_lock).
- OutputStream now uses async drain via Tokio mpsc; sync writers send without blocking.
- Compute is async; worker and tests await compute instead of blocking.

### graph crate

Implements the data structures for graphs and nodes. Nodes are created from functions defined in a function library.
Connections between nodes are represented by `Binding::Output` values.
Data structures like graphs, function libraries, and execution graphs can be serialized to YAML or JSON.

Execution execution is handled by the `execution_graph` module which determines which nodes should run each tick.
Additional modules drive execution and integration:
- `compute` runs active nodes through `FuncLib` lambdas and converts values between data types.
- Function libraries now own lambda invocations directly; `invoke.rs` and the `Invoker`/`UberInvoker` layer were removed.
- Lua function loading now assigns `Func.lambda` directly via `FuncLambda` and asserts input/output counts during invocation.
- `Func.lambda` now uses a `FuncLambda` enum with explicit `None`/`Lambda` states instead of `Option`.
- Lua print output now joins multiple arguments with tab separators while preserving type-specific stringification.
- Lua function loading now holds the function map lock once and iterates inputs/outputs with zip-based loops.
- Lua value conversion now panics with the unsupported Lua value details for easier debugging.
- Lua function parsing now caches input/output counts and includes function+index context in data type errors.
- Lua connection collection now takes and drops the mutex guard in a tighter scope.
- Lua graph wiring now preallocates node storage and validates the final graph with an explicit expect.
- Lua tests now assert ordered multi-value returns and reuse a helper for bound-node names.
- Lua tests now cover invoking the Lua `mult` function directly via the function library.
- ComputeError now stores invocation error text as a `String` for clone-friendly errors.
- Compute invocation now records node errors and returns immediately without cloning the full result.
- Execution graph terminal-node discovery now uses a direct loop instead of iterator chaining.
- Execution graph validation now enforces processing/execution order membership and forbids Processing states.
- `worker` spawns a Tokio thread that executes the graph either once or in a loop and processes events.
- `worker` must be shut down via `Worker::exit()`; dropping a running worker triggers a panic to surface logic errors.
- `worker` event loops return `None` when the message channel closes to signal termination.
- `event` manages asynchronous event loops that send event IDs back to the worker.

Benchmarks:
- `graph/benches/b1.rs` resolves `test_resources` via `CARGO_MANIFEST_DIR`, disables caching on the `sum` node, and benchmarks `ExecutionGraph::new` with Criterion.

Execution graph construction now uses an explicit stack for active-node ordering to avoid deep recursion limits.
Execution graph creation and scheduling are split into clearer phases (node build, propagation, scheduling).
Execution graph propagation asserts nodes were processed before input state evaluation.
Execution graph propagation expects function library entries to exist and asserts input index bounds.
Execution graph build validates graph+func-lib alignment once up front and no longer repeats validation checks in each phase.
Execution graph node collection uses a helper to reuse cached state (invoke cache, output values, binding counts) from the previous execution.
Execution graph update now uses helpers to reset or build execution nodes without duplicating state initialization logic.
Execution node reset logic now lives on `ExecutionNode` to keep update behavior self-contained.
Execution node port resets are centralized in `ExecutionNode::reset_ports_from_func`.
Execution graph node cache now creates missing execution nodes when new graph nodes appear, clears cached outputs if function output arity changes, and rebuilds output binding counts each pass.
Execution graph node cache lookup uses explicit matches to keep the insert path obvious.
Execution graph scheduling asserts when execution node indices are missing.
Execution graph propagation asserts when output bindings reference missing execution nodes.
Execution graph propagation panics with function and node IDs on missing functions.
Execution graph debug assertions include node indices and IDs to speed up diagnosis.
Execution graph visit/output assertions now include index mismatch context for faster debugging.
Execution graph invariant lookups and tests now use `expect` instead of `unwrap_or_else`.
Compute now sorts invocations by `ExecutionNode::invocation_order`, which resets to `u64::MAX` each pass and is set during scheduling.
Compute input value conversion now uses a single loop instead of iterator chains.
Execution graph update/backward/forward comments now describe traversal intent and invariants.
Execution graph now rebuilds the `e_node_idx_by_id` cache each update and uses it for node lookups.
Execution node cache rebuild now drops execution nodes missing from the current graph.
Execution node cache update compacts in-place with swaps and truncation to minimize allocations.
Execution node cache compaction includes inline comments describing the swap-and-truncate flow.
`graph::test_graph()` constructs the standard sample graph (fixed IDs, bindings, const inputs) and validates it; tests now use it directly instead of deserializing a YAML fixture.
`graph::function::test_func_lib()` constructs the standard sample function library in code; tests and benchmarks use it instead of a YAML fixture.
`graph::common::FileFormat` provides YAML/JSON selection and auto-detects file formats by extension for graph/function library loading.
Execution graphs now expose YAML/JSON serialization helpers for roundtrip testing.
Zed debug config adds a CodeLLDB launch task that sets an LLDB breakpoint on `rust_panic`.

## Common Terms

Add shared terminology here as the project evolves.

## Editor Workflow

Add editor workflow details here as they are defined.
The node header cache button is disabled for terminal nodes to prevent toggling cache behavior.
