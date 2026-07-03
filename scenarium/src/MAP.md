# `scenarium` — file & data-structure map

A reference map of the `scenarium` crate: every source file, its role, and the
data structures it defines (with key fields). Companion to `CLAUDE.md` (prose
architecture) and `execution/README.md` (the cache/flatten design). Types are
grouped by concern, not strictly by file.

Two graph representations drive everything:
- **Authoring graph** (`graph.rs`, `subgraph.rs`) — what the editor edits; serializable, identity-only nodes.
- **Execution program** (`execution/program.rs`) — the compiled, flattened, immutable SoA form.

Pipeline: `Graph` + `Library` → **flatten** → `ExecutionProgram` → **plan** → `ExecutionPlan` → **resolve** (cut) → `needed` mask → **execute** → `ExecutionStats`. Cross-run state lives in `Cache` (RAM) and `OutputCache` (disk).

---

## File map

| File | Role |
|------|------|
| `lib.rs` | Crate root: module list + `prelude` (published surface). |
| `graph.rs` | Authoring model: `Graph`, `Node`, `Binding`, `Subscription`, ports, `NodeKind`, `CachePersistence`. |
| `subgraph.rs` | Composite defs: `SubgraphDef`, `SubgraphRef`, `SubgraphEvent`. |
| `function.rs` | `Func` definition + `FuncInput`/`FuncOutput`/`FuncEvent`/`FuncBehavior`/`OutputType`. |
| `special.rs` | `SpecialNode` — built-in nodes recognized by kind, not `FuncId`. |
| `data.rs` | Value model: `DataType`, `StaticValue`, `DynamicValue`, `FsPathConfig`, `CustomValue`/`PendingPreview` traits. |
| `library.rs` | `Library` registry (funcs + shared subgraphs + nominal types) + `TypeDecl`/`TypeEntry`. |
| `value_codec.rs` | `CustomValueCodec` trait + `CachedValue` blob framing (value ↔ bytes). |
| `func_lambda.rs` | `FuncLambda` (async node fn) + `InvokeInput`/`InvokeResult`/`InvokeError`/`OutputUsage`. |
| `event_lambda.rs` | `EventLambda` (async event handler). |
| `macros.rs` | `async_lambda!` — ergonomic `FuncLambda` construction. |
| `context.rs` | `ContextManager` (per-run resources + logs + cancel) + `ContextType`. |
| `common/any_state.rs` | `AnyState` — per-node type-erased mutable state. |
| `common/shared_any_state.rs` | `SharedAnyState` — concurrent event state + `EventStateGuard`. |
| `execution_stats.rs` | `ExecutionStats`, `FlattenMap` (flat↔authoring ids), progress/log types. |
| `execution/` | The compile→plan→execute pipeline (see below). |
| `elements/` | Built-in node libraries: `basic_library`, `worker_events_library`, `fs_watch_library`, `cache_passthrough`. |
| `worker/mod.rs` | `Worker` — tokio task driving updates, runs, event loop; `WorkerMessage`/`WorkerReport`. |
| `testing.rs` | `TestFuncHooks` test fixtures. |

### `execution/` submodules

| File | Role |
|------|------|
| `execution/mod.rs` | `ExecutionEngine` (owns all phases) + `NodeColumn<T>`, `Error`/`RunError`, `RunSeeds`, `ArgumentValues`. |
| `execution/program.rs` | `ExecutionProgram` (immutable SoA) + `ExecutionNode`/`ExecutionInput`/`ExecutionEvent`/`ExecutionBinding`, `NodeIdx`. |
| `execution/flatten/mod.rs` | Subgraph flattening: `Flattener`, `Run`, `Source`, `Pools`. |
| `execution/plan/mod.rs` | Scheduling: `Planner` + `ExecutionPlan`, `NodeVerdict`. |
| `execution/resolve/mod.rs` | Cache-aware cut: `Resolver` + `Resolved`. |
| `execution/executor/mod.rs` | Run loop: `Executor` + `NodeOutcome`, `Readiness`. |
| `execution/cache/mod.rs` | Cross-run RAM cache: `Cache`/`RuntimeSlot`/`ValueCache`/`InvokeSlot`. |
| `execution/digest/mod.rs` | Content digests: `Digest`, `DigestHasher`, `DigestPod`, `FileId`. |
| `execution/output_cache/mod.rs` | Disk persistence policy: `OutputCache`, `Target`. |
| `execution/blob/mod.rs` | On-disk blob read/write (atomic). |
| `execution/event.rs` | `EventRef`, `EventTrigger`. |
| `execution/query.rs` | Read-only projections off the engine (argument values, event triggers). |
| `execution/validate.rs` | Debug-only self-consistency checks. |

---

## Identity types (`id_type!` UUID newtypes)

| Type | File | Names |
|------|------|-------|
| `NodeId` | `graph.rs` | An authoring node (and, preserved, a top-level flat node). |
| `FuncId` | `function.rs` | A registered `Func`. |
| `SubgraphId` | `subgraph.rs` | A composite definition. |
| `TypeId` | `data.rs` | A registered nominal type (`Custom`/`Enum`). Has `from_name` (UUIDv5). |
| `CtxId` | `context.rs` | A `ContextType` token. |
| `NodeIdx(u32)` | `execution/program.rs` | **Not** a UUID: a positional index into `e_nodes`/`slots`/per-node columns. Resolved at flatten. |

`Span { start: u32, len: u32 }` (from `common`) slices the shared input/event/output pools per node.

---

## Authoring model (`graph.rs`, `subgraph.rs`)

```
OutputPort  { node_id: NodeId, port_idx: usize }         // producer side
InputPort   { node_id: NodeId, port_idx: usize }         // consumer side

enum Binding { None | Const(StaticValue) | Bind(OutputPort) }   // what an input is wired to

Subscription { emitter: NodeId, event_idx: usize, subscriber: NodeId }   // one event edge

enum CachePersistence { Memory | Disk }                  // Disk = request (honored iff reproducible)

enum NodeKind {
    Func(FuncId) | Subgraph(SubgraphRef) | Special(SpecialNode)
    | SubgraphInput | SubgraphOutput                     // boundaries, only inside a def
}

Node {                                                   // pure identity; arity derives from kind
    id: NodeId, kind: NodeKind, name: String,
    persist: CachePersistence, disabled: bool,
}

Graph {                                                  // side-table storage (no per-node Vecs)
    nodes:         KeyIndexVec<NodeId, Node>,
    bindings:      BTreeMap<InputPort, Binding>,         // sparse; absent = None
    subscriptions: BTreeSet<Subscription>,               // deduped event edges
    subgraphs:     KeyIndexVec<SubgraphId, SubgraphDef>, // local (private) defs
}

FreshGraph { graph: Graph, id_map: HashMap<NodeId, NodeId> }   // clone-with-fresh-ids result
```

```
enum SubgraphRef { Linked(SubgraphId) | Local(SubgraphId) }    // shared (Library) vs private (Graph)

SubgraphEvent { name: String, emitter: NodeId, emitter_event_idx: usize }   // exposed outgoing event

SubgraphDef {
    id: SubgraphId, name: String, category: String,
    graph:   Graph,               // interior (≤1 SubgraphInput, ≤1 SubgraphOutput)
    inputs:  Vec<FuncInput>,      // interface, port order
    outputs: Vec<FuncOutput>,
    events:  Vec<SubgraphEvent>,
    origin:  Option<SubgraphId>,  // lineage metadata (runtime ignores)
}
```

---

## Functions & lambdas (`function.rs`, `func_lambda.rs`, `event_lambda.rs`, `special.rs`)

```
enum FuncBehavior { Impure (default) | Pure }            // only Pure is content-cacheable

ValueVariant { name: String, value: StaticValue }        // editor pick

FuncInput {
    name: String, required: bool, data_type: DataType,
    const_only: bool,                                    // reject Bind (e.g. cache path)
    default_value: Option<StaticValue>,
    value_variants: Vec<ValueVariant>,                   // pick-or-wire ports
}

enum OutputType { Fixed(DataType) | Wildcard { mirrors: usize } }   // wildcard mirrors an input
FuncOutput { name: String, ty: OutputType }
FuncEvent  { name: String, event_lambda: EventLambda }

Func {                                                   // build via fluent Func::new(..).pure().input(..)…
    id: FuncId, name, category, terminal: bool,
    uncacheable: bool,                                   // node owns its caching (hides persist toggle)
    behavior: FuncBehavior, version: u64,                // version folds into the digest
    description: Option<String>,
    inputs: Vec<FuncInput>, outputs: Vec<FuncOutput>, events: Vec<FuncEvent>,
    required_contexts: Vec<ContextType>,
    lambda: FuncLambda,                                  // #[serde(skip)]
}
```

```
enum OutputUsage { Skip | Needed(u32) }                  // how many consumers read an output this run
InvokeInput { value: DynamicValue }
enum InvokeError { External(anyhow::Error) | Cancelled }
type InvokeResult<T> = Result<T, InvokeError>

enum FuncLambda  { None | Lambda(Arc<dyn AsyncLambdaFn>) }   // async fn(&mut Ctx,&mut AnyState,&Shared,&[In],&[Usage],&mut [Out])
enum EventLambda { None | Lambda(Arc<dyn AsyncEventFn>) }    // async fn(SharedAnyState)

enum SpecialNode { CachePassthrough { bypass: bool } }   // built-in node identified by kind
```

---

## Value model (`data.rs`)

```
enum FsPathMode { ExistingFile (default) | NewFile | Directory }
FsPathConfig { mode: FsPathMode, extensions: Vec<String> }

enum DataType {                                          // port type spec
    Null | Float | Int | Bool | String
    | FsPath(Arc<FsPathConfig>)                          // structural, inline
    | Custom(TypeId) | Enum(TypeId)                      // nominal; metadata on Library
}

enum StaticValue {                                       // serializable authored constant
    Null | Float(f64) | Int(i64) | Bool(bool)
    | String(String) | FsPath(String) | Enum(String)    // NaN-aware Eq
}

enum DynamicValue {                                      // runtime value (NOT Serialize)
    Unbound | Static(StaticValue) | Custom(Arc<dyn CustomValue>)
}

trait CustomValue    { type_id() -> TypeId; gen_preview(..); as_any() }   // app-extension payload
trait PendingPreview { async wait(..) }
trait EnumVariants   { variant_names() -> Vec<String> }                   // blanket over strum
```

---

## Library & codecs (`library.rs`, `value_codec.rs`)

```
enum TypeDecl { Custom { display_name } | Enum { display_name, variants: Vec<String> } }
TypeEntry { decl: TypeDecl, codec: Option<Arc<dyn CustomValueCodec>> }    // codec #[serde(skip)]

Library {                                                // the runtime registry (serializable)
    funcs:     KeyIndexVec<FuncId, Func>,
    subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>,     // shared (linked) defs
    types:     HashMap<TypeId, TypeEntry>,               // nominal-type metadata + disk codecs
}

trait CustomValueCodec { async encode(..) -> Vec<u8>; decode(Vec<u8>) -> Arc<dyn CustomValue> }
enum CachedValue { Unbound | Static(StaticValue) | Custom { type_id: TypeId, blob: Vec<u8> } }   // blob mirror
enum value_codec::Error { Frame | Encode | UnknownType | Decoder }   // crate-internal
// FORMAT_VERSION: u32 — blob framing version, prefixed to every blob.
```

---

## Context & state (`context.rs`, `common/`)

```
ContextType { ctx_id: CtxId, description: String, ctor: Arc<dyn Fn()->Box<dyn Any+Send>> }   // lazy-init token

ContextManager {                                         // per-run resource store
    store:        HashMap<ContextType, Box<dyn Any+Send>>,
    current_node: Option<NodeId>,                        // for log attribution
    logs:         Vec<LogEntry>,
    cancel:       CancelToken,                            // cooperative cancel (cancel_flag() clones it)
}

AnyState       { boxed: Option<Box<dyn Any+Send>> }      // per-node mutable state (downcast get/set)
SharedAnyState { inner: Shared<AnyState> }               // concurrent event state; lock() -> EventStateGuard
```

---

## Execution pipeline (`execution/`)

### Program — the compiled SoA form (`program.rs`)

```
NodeIdx(u32)                                             // positional index into all per-node structures
ExecutionPortAddress { target_idx: NodeIdx, port_idx: usize }
enum ExecutionBinding { None | Const(StaticValue) | Bind(ExecutionPortAddress) }
ExecutionInput { required: bool, binding: ExecutionBinding }
ExecutionEvent { subscribers: Vec<NodeIdx>, lambda: EventLambda }

ExecutionNode {                                          // topology + code, immutable across runs
    id: NodeId, inited: bool (compile scratch),
    terminal: bool, behavior: FuncBehavior,
    persist: bool, special: Option<SpecialNode>,
    inputs: Span, outputs: Span, events: Span,           // slices into the pools below
    func_id: FuncId, func_version: u64,
    lambda: FuncLambda, name: String,
}

ExecutionProgram {                                       // the only serializable runtime artifact
    e_nodes:      KeyIndexVec<NodeId, ExecutionNode>,
    inputs:       Vec<ExecutionInput>,                   // flat pool, sliced by e_node.inputs
    events:       Vec<ExecutionEvent>,                   // flat pool, sliced by e_node.events
    output_types: Vec<DataType>,                         // flat pool, sliced by e_node.outputs
    // n_outputs() is derived = output_types.len()
}
```

### Flatten (`flatten/mod.rs`)

```
Flattener {                                              // reusable compile scratch (owned by engine)
    path: Vec<NodeId>,          // descent path of composite-instance ids
    scope_stack: Vec<u32>,      // parallel FlattenMap scope indices
    seen: HashSet<SubgraphId>,  // recursion guard
    subs: Vec<Subscription>,    // resolved flat event edges (applied after node pass)
    inputs_scratch: Vec<ExecutionInput>,
}
Pools<'a> { inputs, events }                             // the SoA pools being rebuilt
enum Source { Producer { node_id, port_idx } | Const(StaticValue) | None }   // resolved binding target
Run<'a> { … }                                            // one flattening pass (borrows Flattener scratch)
```

### Plan (`plan/mod.rs`) — purely structural

```
enum NodeVerdict { Execute | MissingInputs (default) }   // runnable vs blocked on inputs

ExecutionPlan {
    process_order: Vec<NodeIdx>,          // post-order DFS, deps before consumers
    verdicts:      NodeColumn<NodeVerdict>,
    output_usage:  Vec<u32>,              // per-output consumer counts (0 = skip)
    roots:         Vec<NodeIdx>,          // walk seeds (terminals, subscribers, trigger owners)
}

Planner { color: NodeColumn<Color>, stack: Vec<Visit> }  // reusable DFS scratch
enum Color { White | Gray | Black }
enum VisitCause { Discover | Done }
Visit { e_node_idx: NodeIdx, cause: VisitCause }
```

### Resolve (`resolve/mod.rs`) — cache-aware cut

```
enum Resolved { Reuse | Run (default) }                  // does the node reuse a cache this run?
Resolver {
    resolved: NodeColumn<Resolved>,       // per-node reuse verdict
    needed:   NodeColumn<bool>,           // cut mask handed to the executor
}
```

### Execute (`executor/mod.rs`)

```
enum NodeOutcome {                                       // the single per-node run result column
    Pending (default)
    | Reused                              // served from RAM/disk cache
    | Cut { cached: bool }                // pruned by the cut (its consumers all reused)
    | Ran { secs: f64 }
    | Failed { secs: f64, error: RunError }
    | Skipped { error: RunError }         // upstream errored / blob load failed
}

Executor {
    ctx_manager: ContextManager,
    inputs: Vec<InvokeInput>,                             // per-invoke scratch
    output_usage_scratch: Vec<OutputUsage>,
    outcomes: NodeColumn<NodeOutcome>,
}
enum Readiness { Reuse | InputsUnavailable | Run }        // prepare_node verdict
```

### Cross-run cache (`cache/mod.rs`) — RAM tier

```
enum ValueCache {                                        // one node's cached output (3-state)
    Empty
    | Resident { values: Vec<DynamicValue>, produced_under: Option<Digest> }
    | OnDisk                              // decodable blob exists, not yet loaded
}

RuntimeSlot {                                            // per-node, index-aligned to e_nodes
    id: NodeId, state: AnyState, event_state: SharedAnyState,
    current_digest: Option<Digest>,       // this run's content key (None = impure)
    value: ValueCache,
}
InvokeSlot<'a> { state: &mut AnyState, outputs: &mut Vec<DynamicValue> }   // split borrow for a lambda
Cache { slots: KeyIndexVec<NodeId, RuntimeSlot> }        // reconciled to node set each update
```

### Digests (`digest/mod.rs`)

```
Digest([u8; 32])                                         // 256-bit BLAKE3, cross-machine stable
trait DigestPod { write_le(..) }                         // LE-encode primitives into a hasher
DigestHasher(blake3::Hasher)                             // fluent builder (write_pod/str/digest/fs_path)
FileId { len: u64, mtime_ns: u128 }                      // FsPath external identity
// DOMAIN: &[u8] — hashing-scheme version separator.
```

### Disk persistence (`output_cache/mod.rs`, `blob/mod.rs`)

```
enum Target { Addressed(PathBuf) | Explicit(PathBuf) }   // content-addressed vs explicit path
OutputCache {                                            // the disk policy layer
    library: Arc<Library>,                // codec registry snapshot
    disk_root: Option<PathBuf>,           // None = memory-only
}
enum blob::Error { Encode | Write }                      // real store failure (crate-internal)
```

### Events (`event.rs`)

```
EventRef     { node_id: NodeId, event_idx: usize }       // names one event port of one flat node
EventTrigger { event: EventRef, lambda: EventLambda, state: SharedAnyState }   // one looping task
```

### Engine & shared column (`execution/mod.rs`)

```
NodeColumn<T> { values: Vec<T> }                         // per-node Vec, indexed only by NodeIdx

enum Error    { InvalidGraph | CycleDetected | EventLambdaPanic }    // operation-level failure
enum RunError { Invoke | SkippedUpstream | Cancelled }              // single-node run failure
type Result<T> = std::result::Result<T, Error>

ArgumentValues { inputs: Vec<Option<DynamicValue>>, outputs: Vec<DynamicValue> }
RunSeeds { terminals: bool, event_triggers: bool, events: Vec<EventRef> }   // what to schedule

ExecutionEngine {                                        // owns the whole pipeline
    program:      ExecutionProgram,
    flattener:    Flattener,          flatten_map: Arc<FlattenMap>,
    cache:        Cache,              executor: Executor,
    planner:      Planner,            resolver: Resolver,
    plan:         ExecutionPlan,      output_cache: OutputCache,
}
```

---

## Stats & progress (`execution_stats.rs`)

```
FlattenMap { scopes: Vec<Scope>, leaves: HashMap<NodeId, Leaf> }   // flat id → authoring attribution
  Scope { instance: Option<NodeId>, parent: u32 }        // composite-expansion arena (0 = root)
  Leaf  { scope: u32, interior: NodeId }
Attribution<'a> { … }                                    // iterator: interior id, then enclosing instances

ExecutedNodeStats { node_id: NodeId, elapsed_secs: f64 }
enum RunPhase  { Started { at: Instant } | Finished { elapsed_secs: f64 } }
RunProgress    { nodes: Vec<NodeId>, phase: RunPhase }   // live, pre-resolved via FlattenMap
NodeError      { node_id: NodeId, error: RunError }
enum LogLevel  { Info | Warn | Error }
LogEntry       { node_id: NodeId, level: LogLevel, message: String }

ExecutionStats {                                         // per-run summary
    elapsed_secs: f64,
    executed_nodes: Vec<ExecutedNodeStats>,
    missing_inputs: Vec<InputPort>,
    cached_nodes:   Vec<NodeId>,
    triggered_events: Vec<EventRef>,
    node_errors:    Vec<NodeError>,
    logs:           Vec<LogEntry>,
    flatten:        Arc<FlattenMap>,
    cancelled:      bool,
}
```

---

## Worker (`worker/mod.rs`)

```
enum WorkerReport  { Progress(RunProgress) | Finished(Result<ExecutionStats>) }
enum WorkerMessage {                                     // a Vec<WorkerMessage> = one atomic commit
    Exit | InjectEvents { events }
    | Update { graph, library } | SaveCaches { graph, library } | Clear
    | SetOutputCache(OutputCache)
    | ExecuteTerminals | StartEventLoop | StopEventLoop
    | Sync { reply } | RequestArgumentValues { node_id, reply }
}
struct WorkerExited                                      // send-after-exit error

Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<Vec<WorkerMessage>>,
    event_loop_started: Arc<AtomicBool>,
    cancel: CancelToken,                                 // request_cancel(); reset each run
}

// Internal loop machinery:
LambdaPanic { node_id, message }                         // isolated event-lambda panic
EventLoopHandle { join_handles: Vec<(EventRef, JoinHandle<()>)> }   // spawned event tasks
enum GraphOp     { Clear | Replace(Graph, Arc<Library>) }           // last-write-wins
enum LoopCommand { Start | Stop }
BatchIntent { graph_state, output_cache, save_caches, loop_request,
              execute_terminals, exit, events, syncs, argument_requests }   // scan() reduction target
StopOutcome { … }
```

---

## Published surface (`lib.rs` `prelude`)

Re-exports: `ExecutionEngine`, `Digest`/`DigestHasher`, `OutputCache`, `EventRef`/`EventTrigger`;
`Graph`/`Node`/`NodeId`/`NodeKind`/`Binding`/`Subscription`/`InputPort`/`OutputPort`/`CachePersistence`;
`SubgraphDef`/`SubgraphRef`/`SubgraphEvent`/`SubgraphId`; `Func`/`FuncBehavior`/`FuncId`;
`Library`/`TypeDecl`/`TypeEntry`; `SpecialNode`; `CustomValueCodec`;
`DataType`/`StaticValue`/`DynamicValue`/`CustomValue`/`TypeId`; `ContextType`; `AnyState`/`SharedAnyState`;
`FuncLambda`/`InvokeInput`/`InvokeResult`/`InvokeError`;
`ExecutionStats`/`ExecutedNodeStats`/`FlattenMap`/`RunProgress`/`RunPhase`/`NodeError`/`LogEntry`/`LogLevel`.

Everything under `execution/` except the above (`ExecutionProgram`, `ExecutionNode`, `Cache`, `Planner`,
`Executor`, `Resolver`, `NodeIdx`, …) is `pub(crate)` — internal to the pipeline.
