# Scenarium architecture and simplification review

## Executive summary

Scenarium retains a strong top-level split between authoring graphs, compiled programs, planning, cache-aware resolution, and execution. The remaining correctness risks are concentrated in event-seed validation and cache identity: malformed event seeds can panic the worker, optional disk-cache failures can fail an otherwise valid run, function and codec changes can reuse stale values, and filesystem fingerprints can miss content changes.

The main structural problems are late registry validation, mixed lifecycle state in `ContextManager`, a wide positional lambda ABI, parallel compiled-data pools with distributed invariants, and large execution modules that own several distinct responsibilities. The public surface also exposes worker coordination and execution-internal types that have no external production consumer.

## Current flow

`Compiler` validates an authoring `Graph`, recursively flattens graph instances into an `ExecutionProgram`, resolves output types, and returns a self-contained `CompiledGraph`. `Worker` reduces batches of public `WorkerMessage`s into an intent, installs shared compiled state, acknowledges the installation through its ordered report stream, plans roots, prepares resource stamps, resolves digests and cache liveness, executes surviving nodes, and reports execution identities. `RuntimeCache` owns persistent node state, resident outputs, disk availability state, digest stamping, codec dispatch, hydration, persistence, reclamation, and RAM accounting.

## Critical

- [ ] **Injected event seeds bypass program validation.** Exact node seeds return `Error::NodeSeedNotFound` when their `ExecutionNodeId` is absent, but public event seeds index the installed node map and event slice directly. A missing execution node or out-of-range event index can panic the worker task (`src/execution/plan/mod.rs:243-267`, `src/worker/protocol.rs:22-29`).

- [ ] **A failed disk-cache read aborts a valid run once.** Resolution treats a matching header as a reusable value and removes its producer cone, but hydration later clears a corrupt, incompatible, or concurrently deleted blob and returns `false`. The consumer then receives `RunError::InputLoadFailed`; the producer is recomputed only on a later run, so enabling the optional cache changes whether the current execution succeeds (`src/execution/cache/mod.rs:464-570`, `src/execution/executor/mod.rs:186-202`, `src/execution/mod.rs:166-171`).

- [ ] **Disk-cache presence checks block the async worker.** Blob coverage probes synchronously open, read, and inspect files during reuse resolution and before stores, while failed hydration also deletes files synchronously. Disk-backed graphs can therefore stall the worker once per cache probe despite body reads and writes using the blocking pool (`src/execution/disk_store/mod.rs:57-69`, `src/execution/disk_store/mod.rs:140-150`, `src/execution/disk_store/mod.rs:175-198`, `src/execution/cache/mod.rs:481-497`).

- [ ] **Function implementations have no cache revision.** `Func` carries a stable `FuncId` and executable lambda but no implementation revision, while `node_digest` keys pure results by the global domain, `FuncId`, output signature, and inputs. Changing value logic under the same persisted identity and signature can silently reuse RAM or disk values produced by the old implementation (`src/node/definition.rs:186-212`, `src/execution/digest/mod.rs:15-21`, `src/execution/digest/mod.rs:234-288`).

- [ ] **Custom codec revisions are absent from blob identity.** Type registration records only an optional codec, and disk-hit eligibility checks only whether a codec is present. A breaking codec change is discovered during lazy decoding rather than during cache lookup, which can select an incompatible blob and then fail the run through the cache-read path (`src/library.rs:44-55`, `src/execution/disk_store/mod.rs:103-112`, `src/execution/digest/mod.rs:35-36`, `../lens/src/image/codec/mod.rs:18-25`, `../lens/src/image/codec/mod.rs:82-91`).

- [ ] **Filesystem resource fingerprints can miss content changes.** Files are identified only by length and modification time; directories hash only immediate entry names and metadata. Same-size edits with an unchanged timestamp and nested-directory content edits can therefore reuse stale cached results, while the execution documentation claims an opt-in content hash that does not exist (`src/execution/resource/mod.rs:22-126`, `src/execution/digest/mod.rs:22-29`, `src/execution/README.md:199-203`).

## High

- [ ] **`Library` admits functions without implementations.** `Func` defaults to `FuncLambda::None`, and `Func::validate` does not reject that state. The invalid registration survives compilation and becomes a per-node `RunError::MissingLambda`, leaving a host configuration error on the execution hot path (`src/node/definition.rs:186-224`, `src/node/definition.rs:313-349`, `src/node/lambda.rs:93-124`, `src/execution/mod.rs:146-171`).

- [ ] **Registry collisions have incompatible semantics.** Function and shared-graph insertion silently replace an existing identity, while duplicate type registration panics; `Library::merge` applies all three behaviors implicitly. Accidental function replacement is particularly dangerous because `FuncId` is also part of persistent cache identity (`src/library.rs:148-175`, `src/library.rs:214-224`).

- [ ] **`TypeEntry` represents invalid enum attachment states.** Its public `decl`, `codec`, and `stamper` fields allow enum types to carry runtime attachments, and registration rejects those combinations only through fallible validation followed by a panic. The public model therefore permits states the registry cannot accept (`src/library.rs:13-64`, `src/library.rs:168-175`).

- [ ] **Context lookup is dynamically typed while context declarations are advisory.** `ContextType` does not retain its value type, so callers can request the wrong `T` and panic during downcast. Its public `description` is unread, and `Func::required_contexts` has production writers but no production reader, exposing metadata that does not enforce or describe runtime behavior (`src/runtime/context.rs:16-20`, `src/runtime/context.rs:105-137`, `src/node/definition.rs:207-212`, `src/node/definition.rs:303-305`).

- [ ] **`ContextManager` combines state with different lifetimes.** It owns persistent typed resources together with the current execution node, per-run logs, and cancellation state. Codecs receive this full run-management object merely to access persistent resources, coupling cache serialization to execution attribution and cancellation state (`src/runtime/context.rs:23-79`, `src/runtime/context.rs:120-137`, `src/execution/codec.rs:31-39`).

## Medium

- [ ] **The lambda ABI is wide, positional, and wrapper-heavy.** Every async function receives six ordered borrows, including a mutable slice of one-field `InvokeInput` wrappers. The ABI couples all lambdas, macros, and executor frame mechanics to the same argument order and requires an additional input representation that carries no behavior (`src/node/lambda.rs:50-89`, `src/node/macros.rs:1-25`, `src/execution/executor/mod.rs:119-128`, `src/execution/executor/mod.rs:186-215`, `src/execution/executor/mod.rs:430-448`).

- [ ] **`DiskStore` depends on the entire `Library` and embeds its disabled state.** It retains graphs, functions, editor metadata, and type declarations solely to look up codecs, while `disk_root: Option<PathBuf>` makes every target lookup handle disabled storage. This couples persistence to unrelated registry replacement and broadens the state reachable from cache I/O (`src/execution/disk_store/mod.rs:29-40`, `src/execution/disk_store/mod.rs:73-112`, `src/execution/codec.rs:80-125`).

- [ ] **`Func::uncacheable` advertises execution semantics that Scenarium does not enforce.** No execution path reads the flag; its only current core producer is `RunSinks`, and Darkroom alone interprets it to hide cache controls. Function metadata and actual cache behavior can therefore disagree (`src/node/definition.rs:193-202`, `src/node/definition.rs:248-253`, `src/elements/run_sinks.rs:25-45`, `../darkroom/src/gui/scene.rs:268-316`).

- [ ] **The compiled program's parallel pools distribute structural invariants across the pipeline.** Each `ExecutionNode` stores spans into shared input, event, output-type, and pin vectors. Flattening maintains running offsets, output ownership is reconstructed later, and validation must cross-check every range, so a local node-shape change affects flattening, type resolution, planning, caching, and execution (`src/execution/program.rs:21-39`, `src/execution/program.rs:77-168`, `src/execution/flatten/mod.rs:52-110`, `src/execution/flatten/mod.rs:280-327`, `src/execution/validate.rs:16-68`).

- [ ] **`execution/mod.rs` owns unrelated execution layers.** The module root defines shared pool containers and aliases, public execution errors, run seeds, engine installation, orchestration, cache reporting, and resident-cache persistence. These responsibilities obscure the boundaries already represented by the execution submodules (`src/execution/mod.rs:20-120`, `src/execution/mod.rs:122-215`, `src/execution/mod.rs:217-379`).

- [ ] **Cache and executor modules each contain several independent state machines.** `cache/mod.rs` combines coverage encoding, snapshots, slot state, invocation mutation, disk policy, digest state, hydration, persistence, reclamation, and RAM reporting in 680 lines. `executor/mod.rs` combines frame hydration, reclamation, run scheduling, invocation, outcome classification, event reporting, and statistics projection in 638 lines, making local changes depend on distant invariants (`src/execution/cache/mod.rs:31-680`, `src/execution/executor/mod.rs:41-638`).

## Low

- [ ] **The public worker API exposes its raw coordination protocol.** Callers construct `Update`, `Run`, event-loop, synchronization, storage, clear, and exit messages directly, while batching silently coalesces and orders them. Correct sequencing and installed-program assumptions therefore remain obligations of every caller rather than properties of the `Worker` API (`src/worker/mod.rs:23-85`, `src/worker/protocol.rs:12-32`, `src/worker/batch.rs:69-100`).

- [ ] **Execution-internal transport and validation remain public.** `RunEvent` is re-exported even though it is used only between the executor and worker, and `Graph::validate_for_execution` plus its debug wrapper are public despite having no external production caller. This enlarges the supported API with implementation details whose invariants are owned internally (`src/lib.rs:22-34`, `src/execution/report.rs:30-34`, `src/graph/validate.rs:275-295`).

- [ ] **Scenarium carries direct dependencies for narrow convenience APIs.** `strum` supports only the blanket enum-variant bridge, while `uuid` supports an otherwise unused name-derived `TypeId` helper and conversion of execution IDs for cache filenames. Both dependencies enlarge Scenarium's direct surface for behavior with no broader production use in the crate (`Cargo.toml:11-21`, `src/data/type_system.rs:5-29`, `src/library.rs:95-102`, `src/execution/identity.rs:40-42`, `src/execution/disk_store/mod.rs:85-100`).

- [ ] **The crate mixes standard-library and `hashbrown` maps without a documented boundary.** The two implementations appear within the same registries and graph modules, producing aliases and conversion friction even though production code uses no `hashbrown`-specific API (`src/library.rs:1-11`, `src/graph/wiring.rs:1-4`, `src/graph/clone.rs:1-4`, `Cargo.toml:19`).

- [ ] **Function declarations couple editor metadata to executable registration.** `Func`, `FuncInput`, and `FuncOutput` combine runtime contracts with categories, descriptions, display labels, default literals, and picker variants. Headless compilation and execution consequently retain editor-only data, and metadata and implementation changes share one registry lifecycle (`src/node/definition.rs:21-69`, `src/node/definition.rs:152-212`).
