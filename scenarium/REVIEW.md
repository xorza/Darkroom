# Scenarium architecture and simplification review

## Executive summary

Scenarium has a clear compile → plan → resolve → execute pipeline, but several
invariants are split across parallel representations and lifecycle layers. The
highest-impact worker problem is that `send_many` does not establish the batch
boundary its callers rely on.

The authoring model has similar duplication between declarations, boundary
nodes, normalization, validation, and detached undo state. At runtime,
execution-node state, typed contexts, disk codecs, per-run maps, reporting, and
the lambda ABI each carry more ownership or coordination state than their
responsibility requires. The result is avoidable full-graph work for targeted
runs, repeated traversal bookkeeping, and APIs whose metadata does not enforce
the behavior it describes.

## Current flow

`Compiler` validates an authored `Graph`, recursively flattens composite
instances into an `ExecutionProgram`, resolves output types, and returns a
`CompiledGraph`. `WorkerTask` opportunistically reduces ready messages and
events into a `BatchIntent`, installs compiled state, plans roots, prepares
filesystem stamps, resolves cache-aware liveness, executes surviving nodes, and
publishes progress and completion snapshots. `RuntimeCache` keeps output
snapshots, function state, event state, digests, and the disk store across runs
and reconciles them by flattened execution identity when a new program is
installed.

## High: Worker lifecycle

- [ ] **`Worker::send_many` does not create the worker commit boundary its API consumers rely on.** It sends messages one at a time, while the worker reduces only whichever messages happen to be ready when `recv_many` wakes (`src/worker/mod.rs:52-60`, `src/worker/task.rs:131-161`, `src/worker/batch.rs:39-72`). The worker can process the first message before the remaining sends arrive, so logically coupled operations can be split across intents; Darkroom explicitly treats update, eviction, and event-loop stop as one commit (`../darkroom/src/core/worker.rs:65-75`). That split can rebuild or run an event loop between the update and the stop/eviction that was meant to accompany it.

## High: Authoring and compilation invariants

- [ ] **Output normalization destroys authored output descriptions.** `FuncOutput` stores a description, and input normalization preserves the full existing `FuncInput`, but output normalization reconstructs every entry from only its name and inferred type before replacing `graph.outputs` (`src/node/definition.rs:151-174`, `src/graph/normalize/mod.rs:105-113`, `src/graph/normalize/mod.rs:125-186`). Any local-graph output description makes the reconstructed interface differ and is silently erased during normalization.

- [ ] **Normalization covers only part of the graph state that validation later treats as structural.** The pruning pass examines bindings and subscriptions but not pinned outputs or exposed events, and `subscription_live` does not check that the subscriber exists (`src/graph/normalize/mod.rs:38-50`, `src/graph/normalize/mod.rs:67-74`). Validation rejects stale pins, exposed events, and missing subscribers afterward (`src/graph/validate.rs:165-224`), so library or interface evolution can leave a normalized document structurally invalid and uncompilable.

- [ ] **Function registration accepts defaults that immediately create invalid nodes.** `FuncInput` carries an unconstrained default and picker variants, while `Func::validate` checks neither and `Library::add` treats that validation as the registration gate (`src/node/definition.rs:50-68`, `src/node/definition.rs:338-382`, `src/library.rs:130-137`). `Graph::add_func_node` installs the unchecked default as a binding, which graph validation can then reject (`src/graph/mod.rs:429-442`, `src/graph/validate.rs:154-162`). A valid registered function therefore does not imply that its standard constructor produces a valid authored node.

- [ ] **Composite-interface validity depends on an unenforced boundary-node convention.** A graph stores its interface separately from optional `GraphInput` and `GraphOutput` nodes; validation enforces at most one boundary but not the boundary required by a nonempty interface (`src/graph/mod.rs:136-149`, `src/graph/mod.rs:221-268`, `src/graph/validate.rs:38-101`). Normalization simply skips a side whose boundary is absent (`src/graph/normalize/mod.rs:125-129`, `src/graph/normalize/mod.rs:157-161`), so a graph can validate with externally visible ports that have no interior counterpart.

- [ ] **The advertised nesting cap neither exists in release builds nor protects the recursive validation that runs first.** Compilation validates the complete graph tree before flattening, while the only depth check is a `debug_assert!` inside recursive emission (`src/execution/compile.rs:126-140`, `src/graph/validate.rs:227-259`, `src/execution/flatten/mod.rs:31-33`, `src/execution/flatten/mod.rs:176-228`). Deep acyclic input therefore has build-profile-dependent behavior and can exhaust the stack before or during compilation.

## Medium: Cross-run state ownership

- [ ] **Program installation preserves function and event state solely because an execution ID still exists.** A runtime slot combines outputs with `AnyState` and `SharedAnyState`; installation replaces the compiled program and `reconcile` retains the entire slot for every matching ID without comparing function identity, version, or signature (`src/execution/cache/slot.rs:54-66`, `src/execution/engine/mod.rs:77-90`, `src/execution/cache/runtime/mod.rs:116-125`). Execution IDs are derived from authoring paths, so a changed function implementation can inherit state owned by the previous implementation even though output digests correctly invalidate its values (`src/execution/identity.rs:17-38`).

- [ ] **Context identity and payload type are independent runtime choices.** `ContextType::new<T>` erases `T`, `ContextManager::get<U>` accepts an unrelated requested type and panics on downcast, and equality and hashing consider only the UUID (`src/runtime/context.rs:91-138`). Separately declared handles with one ID alias regardless of constructor or payload type, making ordinary context access depend on manually keeping three independent facts consistent.

- [ ] **Context declarations are advisory while `ContextManager` mixes persistent and per-run lifetimes.** `Func::required_contexts` has production writers but no production reader, and `ContextType::description` does not participate in execution (`src/node/definition.rs:208-237`, `src/node/definition.rs:328-330`, `src/runtime/context.rs:16-20`). The manager simultaneously owns persistent resources, current-node attribution, logs, and cancellation, and custom codecs receive that entire object merely to access resources during encoding (`src/runtime/context.rs:23-37`, `src/execution/codec.rs:19-30`, `src/execution/disk_store/format/mod.rs:56-62`). Declaration metadata therefore does not enforce availability, while cache serialization is coupled to invocation bookkeeping.

- [ ] **`DiskStore` retains the entire `Library` for codec lookup.** The store owns an `Arc<Library>` even though format operations use only the custom-type codecs (`src/execution/disk_store/mod.rs:24-30`, `src/execution/disk_store/mod.rs:103-112`, `src/execution/disk_store/mod.rs:135-142`, `src/execution/disk_store/format/mod.rs:91-99`, `src/execution/disk_store/format/mod.rs:239-245`). Cache I/O consequently retains unrelated functions, shared graphs, and editor-facing type metadata, tying cache-store replacement to changes in the broader registry.

## Medium: Per-run orchestration complexity

- [ ] **Targeted runs still rebuild full-program state across four node hash maps and two output columns.** Planning, DFS coloring, resolution, and execution outcomes each clear and repopulate state from every installed node before walking the reachable schedule; resolution also initializes every output entry (`src/execution/plan/mod.rs:91-105`, `src/execution/plan/mod.rs:133-144`, `src/execution/resolve/mod.rs:60-64`, `src/execution/resolve/mod.rs:105-116`, `src/execution/executor/mod.rs:113-123`). A one-node preview therefore pays whole-graph hashing and initialization costs while the same identities and lifecycle states are duplicated across pipeline phases.

- [ ] **Flattening represents its current location only as a mutable ID path and repeatedly reconstructs the current graph from the root.** Every `current()` call walks all enclosing instances again, while binding and event resolution manually balance path pushes and pops across recursive calls (`src/execution/flatten/mod.rs:117-174`, `src/execution/flatten/mod.rs:327-345`, `src/execution/flatten/mod.rs:386-395`, `src/execution/flatten/mod.rs:419-460`). Compile work grows with nesting depth for each resolved edge, and the twelve-field traversal frame carries path/scope synchronization invariants throughout the pass.

- [ ] **Resolution serially hydrates every reusable disk frontier before execution starts.** The reverse sweep awaits `check_reuse` for each live node, and a disk hit immediately installs the complete decoded snapshot as resident (`src/execution/resolve/mod.rs:127-140`, `src/execution/resolve/mod.rs:161-195`, `src/execution/cache/runtime/mod.rs:227-253`). Independent disk reads accumulate into startup latency, and all accepted snapshots can occupy RAM together before the first lambda runs.

- [ ] **Live reporting adds an unbounded same-task relay followed by copy-on-write status snapshots.** Each run creates an unbounded channel and polls it beside the engine future while the executor synchronously queues progress and pinned payloads (`src/worker/task.rs:317-347`, `src/execution/executor/mod.rs:248-255`, `src/execution/executor/mod.rs:330-338`, `src/execution/executor/value_flow.rs:58-87`). Ready-heavy runs can buffer the whole event stream and publish “live” updates only after completion; if an earlier report remains queued, `Arc::make_mut` deep-clones its vectors immediately before clearing them for the next update (`src/worker/status.rs:78-99`, `src/worker/status.rs:188-191`).

- [ ] **The lambda ABI and executor call chain are wide, positional, and wrapper-heavy.** Every function receives six ordered borrows, including a mutable slice of one-field `InvokeInput` wrappers, and the macro duplicates four patterns around that exact order (`src/node/lambda.rs:63-102`, `src/node/lambda.rs:125-150`, `src/node/macros.rs:1-25`). `Executor::run` separately suppresses its eight-argument warning and builds another multi-field execution frame (`src/execution/executor/mod.rs:90-106`, `src/execution/executor/mod.rs:125-133`). Changes to invocation state therefore propagate through the public ABI, macros, executor, and every registered lambda.

## Medium: Parallel authoring representations

- [ ] **The single special node creates a parallel dispatch path throughout the ordinary function model.** `SpecialNode` contains only `RunSinks` and immediately maps it back to a normal `Func`, yet `NodeKind` gives it a distinct serialized variant and every input, output, event, flattening, and planning path carries special-node branches (`src/node/special.rs:19-38`, `src/graph/mod.rs:136-149`, `src/graph/query.rs:30-73`, `src/execution/flatten/mod.rs:197-237`, `src/execution/plan/mod.rs:266-283`). One planner-specific behavior therefore expands the common authoring and execution state space across the crate.

- [ ] **`DetachedNode` duplicates the graph’s ordered side-table representation with manually validated vectors.** `Graph` already represents bindings, subscriptions, and pins with ordered map/set types, but detachment converts them into public vectors, manually rechecks touching, ordering, and uniqueness, then converts them back during attachment (`src/graph/mod.rs:244-263`, `src/graph/wiring.rs:44-101`, `src/graph/wiring.rs:146-190`). The second serializable representation admits malformed states and carries a validation/conversion path solely to reconstruct invariants the canonical containers already encode.
