# Scenarium architecture and simplification review

## Executive summary

Scenarium retains a strong top-level split between authoring graphs, compiled programs, planning, cache-aware resolution, and execution. Filesystem identity remains intentionally approximate, but runtime eviction now removes selected flattened cache cones from RAM and disk without mutating the authored graph. Eager hydration and pin delivery still make disk-backed reuse consume more RAM and I/O than the live schedule requires.

The main structural problems are inconsistent registry semantics, mixed lifecycle state in `ContextManager`, and a wide positional lambda ABI. The public surface also exposes worker coordination and execution-internal types that have no external production consumer.

## Current flow

`Compiler` validates an authoring `Graph`, recursively flattens graph instances into an `ExecutionProgram`, resolves output types, and returns a self-contained `CompiledGraph`. `Worker` reduces batches of public `WorkerMessage`s into an intent, installs shared compiled state, acknowledges the installation through its ordered report stream, plans roots, prepares resource stamps, resolves digests and cache liveness, executes surviving nodes, and reports execution identities. `RuntimeCache` owns persistent node state, resident outputs, disk availability state, digest stamping, codec dispatch, hydration, persistence, reclamation, and RAM accounting. Cache demand tests whether a blob's coverage is sufficient but does not select payloads, so an accepted blob hydrates as a complete snapshot. A node with a complete digest can hydrate during resolution; a node waiting on a bound resource can instead hydrate after its producers settle. Those paths are mutually exclusive, so one node's blob is read at most once per run. Authoring nodes carry cache-storage mode only; explicit eviction maps authored ids to flattened occurrences and their downstream data consumers, then removes those runtime outputs and disk files.

## Critical: User-controlled cache validity

- [x] **Filesystem cache validity has an explicit runtime escape hatch.** Metadata stamping remains deliberately cheap and can miss same-size edits hidden by mtime granularity, but every reusable output-bearing node now exposes an eviction chip. It resolves graph instances through flattened attribution, clears the selected node and transitive data consumers from RAM, deletes their node-keyed disk blobs, and makes the frontend discard its cache projections without dirtying the document. Function-owned caches such as `Build Masters` remain outside Scenarium's cache ownership.

## High: Registry and runtime validity

- [x] **`Library` rejects functions without implementations.** `Func::validate` reports `MissingLambda`, and every registration path (`add`, `from`, and `merge`) passes through that validation before inserting each function. Declaration-only tests attach an explicit stub implementation rather than constructing a runtime-invalid library (`src/node/definition.rs`, `src/library.rs`).

- [ ] **Registry collisions have incompatible semantics.** Function and shared-graph insertion silently replace an existing identity, while duplicate type registration panics; `Library::merge` applies all three behaviors implicitly. Accidental function replacement is particularly dangerous because `FuncId` is also part of persistent cache identity (`src/library.rs:148-175`, `src/library.rs:214-224`).

- [ ] **`TypeEntry` represents invalid enum attachment states.** Its public `decl`, `codec`, and `stamper` fields allow enum types to carry runtime attachments, and registration rejects those combinations only through fallible validation followed by a panic. The public model therefore permits states the registry cannot accept (`src/library.rs:13-64`, `src/library.rs:168-175`).

- [ ] **Context lookup is dynamically typed while context declarations are advisory.** `ContextType` does not retain its value type, so callers can request the wrong `T` and panic during downcast. Its public `description` is unread, and `Func::required_contexts` has production writers but no production reader, exposing metadata that does not enforce or describe runtime behavior (`src/runtime/context.rs:16-20`, `src/runtime/context.rs:105-137`, `src/node/definition.rs:207-212`, `src/node/definition.rs:303-305`).

- [ ] **`ContextManager` combines state with different lifetimes.** It owns persistent typed resources together with the current execution node, per-run logs, and cancellation state. Codecs receive this full run-management object merely to access persistent resources, coupling cache serialization to execution attribution and cancellation state (`src/runtime/context.rs:23-79`, `src/runtime/context.rs:120-137`, `src/execution/codec.rs:31-39`).

## Medium: Cache lifecycle

- [ ] **Resolution hydrates every reusable disk frontier before execution starts.** The reverse resolution sweep awaits `check_reuse` for each live node, and a disk hit immediately installs the complete snapshot as resident. Values needed only by late consumers therefore occupy RAM throughout the earlier execution prefix, making peak memory scale with all disk hits selected for the run rather than with the executor's active working set (`src/execution/resolve/mod.rs:121-134`, `src/execution/resolve/mod.rs:159-195`, `src/execution/cache/mod.rs:352-378`, `src/execution/mod.rs:304-330`).

- [ ] **Unchanged disk-backed pins are rehydrated and redelivered on every run.** Pinned roots and ports always seed output demand without tracking whether the host already received the same digest, so a `Disk`-mode hit is read, cloned into a `PinnedOutputs` event, and released once it has no readers on every matching run. Eventless execution retains the same pin demand and disk read even though delivery returns immediately, creating wholly unused I/O for that call path (`src/execution/resolve/mod.rs:66-83`, `src/execution/resolve/mod.rs:182-187`, `src/execution/cache/mod.rs:352-378`, `src/execution/executor/mod.rs:129-162`, `src/execution/executor/mod.rs:206-210`, `src/execution/mod.rs:282-332`).

- [ ] **Late cache reuse can obsolete an upstream cone only after that cone has executed.** A node whose digest depends on an unreadable bound resource remains `Run` during resolution and keeps every producer live; the producer-first executor runs that cone before re-stamping the consumer, discovering its disk hit, and abandoning the now-unused reads. A legitimate late hit can therefore spend the full CPU and memory cost of inputs that the cached consumer never reads (`src/execution/resolve/mod.rs:137-152`, `src/execution/resolve/mod.rs:159-195`, `src/execution/executor/mod.rs:344-386`).

## Medium: Runtime APIs

- [ ] **The lambda ABI is wide, positional, and wrapper-heavy.** Every async function receives six ordered borrows, including a mutable slice of one-field `InvokeInput` wrappers. The ABI couples all lambdas, macros, and executor frame mechanics to the same argument order and requires an additional input representation that carries no behavior (`src/node/lambda.rs:50-89`, `src/node/macros.rs:1-25`, `src/execution/executor/mod.rs:119-128`, `src/execution/executor/mod.rs:186-215`, `src/execution/executor/mod.rs:430-448`).

- [ ] **`DiskStore` depends on the entire `Library` and embeds its disabled state.** It retains graphs, functions, editor metadata, and type declarations solely to look up codecs, while `disk_root: Option<PathBuf>` makes every target lookup handle disabled storage. This couples persistence to unrelated registry replacement and broadens the state reachable from cache I/O (`src/execution/disk_store/mod.rs:29-40`, `src/execution/disk_store/mod.rs:73-112`, `src/execution/codec.rs:80-125`).

- [ ] **`Func::uncacheable` advertises execution semantics that Scenarium does not enforce.** No execution path reads the flag; its only current core producer is `RunSinks`, and Darkroom alone interprets it to hide cache controls. Function metadata and actual cache behavior can therefore disagree (`src/node/definition.rs:193-202`, `src/node/definition.rs:248-253`, `src/elements/run_sinks.rs:25-45`, `../darkroom/src/gui/scene.rs:268-316`).

## Medium: Execution structure

- [x] **Compiled port storage is packed without parallel output invariants.** `ExecutionNode` stores distinct typed ranges into shared input, output-metadata, and event pools, preserving one reusable vector per payload category instead of allocating vectors per node. A pool owns range creation and flattening appends directly into it, eliminating distributed offset bookkeeping. Output type and pin state now occupy one `ExecutionOutput` entry, so their lengths and indices cannot drift; type resolution uses node-local output addresses instead of reconstructing ownership or staging another type vector, and validation has one output range to check (`src/execution/program/mod.rs`, `src/execution/program/pool.rs`, `src/execution/flatten/mod.rs`, `src/execution/validate.rs`).

- [x] **The execution root is only pipeline documentation and module declarations.** Errors, seeds, compiled-program storage, engine orchestration, and reporting now live in their responsibility modules (`src/execution/mod.rs`, `src/execution/error.rs`, `src/execution/seeds.rs`, `src/execution/program/`, `src/execution/engine/`, `src/execution/report.rs`).

- [x] **Cache and executor responsibilities are split at state-machine boundaries.** Cache slot/value state lives in `cache/slot.rs` while cross-run policy and persistence live in `cache/runtime/`. Executor value flow and reclamation live in `executor/value_flow.rs`, outcome projection in `executor/outcomes.rs`, and the run loop remains in `executor/mod.rs` (`src/execution/cache/`, `src/execution/executor/`).

## Low: Public surface and dependency boundaries

- [ ] **The public worker API exposes its raw coordination protocol.** Callers construct `Update`, `Run`, event-loop, synchronization, storage, clear, and exit messages directly, while batching silently coalesces and orders them. Correct sequencing and installed-program assumptions therefore remain obligations of every caller rather than properties of the `Worker` API (`src/worker/mod.rs:23-85`, `src/worker/protocol.rs:12-32`, `src/worker/batch.rs:69-100`).

- [ ] **Execution-internal validation remains public.** `Graph::validate_for_execution` and its debug wrapper are public despite having no external production caller. This enlarges the supported API with implementation details whose invariants are owned internally (`src/graph/validate.rs:275-295`).

- [ ] **Scenarium carries direct dependencies for narrow convenience APIs.** `strum` supports only the blanket enum-variant bridge, while `uuid` supports an otherwise unused name-derived `TypeId` helper and conversion of execution IDs for cache filenames. Both dependencies enlarge Scenarium's direct surface for behavior with no broader production use in the crate (`Cargo.toml:11-21`, `src/data/type_system.rs:5-29`, `src/library.rs:95-102`, `src/execution/identity.rs:40-42`, `src/execution/disk_store/mod.rs:85-100`).

- [ ] **The crate mixes standard-library and `hashbrown` maps without a documented boundary.** The two implementations appear within the same registries and graph modules, producing aliases and conversion friction even though production code uses no `hashbrown`-specific API (`src/library.rs:1-11`, `src/graph/wiring.rs:1-4`, `src/graph/clone.rs:1-4`, `Cargo.toml:19`).

- [ ] **Function declarations couple editor metadata to executable registration.** `Func`, `FuncInput`, and `FuncOutput` combine runtime contracts with categories, descriptions, display labels, default literals, and picker variants. Headless compilation and execution consequently retain editor-only data, and metadata and implementation changes share one registry lifecycle (`src/node/definition.rs:21-69`, `src/node/definition.rs:152-212`).
