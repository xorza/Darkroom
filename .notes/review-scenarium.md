# Review: scenarium

Scope: `scenarium/src`, read with its callers in `darkroom`, `lens` and `common`. The lens that
weighed most: data structures, how canonical, idiomatic and cheap they are. The suite is 375 tests
in 0.07 s, the slowest 55 ms (`worker::pause_gate::tests::overlapping_guards_…`, a wall-clock
timeout).

A temporary proposal: as each batch lands, its lasting decisions fold into `scenarium`'s module
docs and `AGENTS.md`, and the batch is deleted here.

# Part 1 — Proposal

## What is already the right shape

Named so no batch touches it.

- **`CompiledGraph`** — flat columns under typed `u32` indices, `Span` runs for a node's ports and
  an event's subscribers, no per-node `Vec`, `String` or map, a sorted id column that is its own
  reverse lookup, no mutators. The shape of cranelift-entity's `PrimaryMap` ("a primary mapping
  `K -> V` allocating dense entity references") and rustc's `IndexVec`.
- **`Col` / `Span` / `IdxSet` / `idx_type!`** — the canonical typed dense-index containers; per-run
  columns reset in place. They needed tightening only.
- **Forward wires only in the program**, reverse edges derived per eviction by a counting-sort CSR;
  topological order derived per run over the seeded cone.
- **`Graph`'s tables** — `BTreeMap`/`BTreeSet` keyed by id, bindings keyed by the consumer input
  with no stored reverse index, ghosts in tables apart from the live ones. A slot map would need an
  id → slot map beside it, a second index; a stored out-edge index would be a second source.
- **`Mailbox`** — a coalescing slot under a `Mutex` with a `Notify`, never held across an await.
- **The digest encoding** — fixed-width or length-prefixed fields, a tag before each input, no
  collision by concatenation.
- **`NodeKind` / `BodySource` / `Boundary` / `NodeDecl`, `GhostBinding` / `GhostSource`,
  `InputState` / `SubscriptionState`, `Delivery`, `ExecBinding`, `NodeOutcome`, `RootFlags`** —
  closed sum types with no invalid combination reachable.
- **The self-check error enums** (`CompiledGraphValidationError`, `RunScheduleValidationError`,
  `InstallValidationError`) stay: their doc comments record the choice, a value a test can match
  on. Only the holes in what they checked were findings.

## Root causes

4. **The document's rules are written where they are used, not once.** "May this graph hold this
   bind" is assembled by hand at three host sites; the live wiring is two loose fields where the
   ghost wiring is a type.
5. **Declarations are assembled through types that never close.** `Library` is at once the
   registration surface and the registry every frame reads; `Func` and `FuncBuilder` write one
   field list five times; an event is declared in two shapes; what a lambda wrote is never held to
   its port's type.
6. **Leaf modules import the model.** `graph ⇄ library`, `graph ⇄ runtime`, `graph::func ⇄
   graph::node`, `node ⇄ scope`, `scope ⇄ graph_set`, `instance_path ⇄ program_id`, and cycles
   inside `data`.

## Bugs

Each is traced to its code; the batch that fixes it carries its test.

| # | Where | Input → wrong result |
|---|---|---|
| B12 | `graph/value_domain/float_span.rs:90-116` | `FloatSpan::at_least(0.0).contains(f64::INFINITY)` is `true` for a type documented as finite; `between(-f64::MAX, f64::MAX).fraction(f64::MAX)` is `Some(NaN)`. |
| B13 | `data/const_value.rs:156` | `Display` prints a float as `{:.4}`: `0.00004` reads `0.0000`, a zero that hides a nonzero value, shown through `DynamicValue`'s `Display` on a preview card. |

## Target

### Data structures

Types not named here stay as they are.

#### Containers — `containers/col/mod.rs`

```rust
macro_rules! idx_type {            // the field loses `$vis`
    ($(#[$meta:meta])* $vis:vis struct $name:ident, $what:literal) => { /* struct $name(u32); */ };
}

pub(crate) struct Span<I> { start: u32, len: u32, space: PhantomData<I> }
impl<I> Span<I> {
    pub(crate) const fn start(self) -> u32;
    pub(crate) const fn len(self) -> usize;
}

impl<I, T: Clone> Clone for Col<I, T> {
    fn clone(&self) -> Self;
    fn clone_from(&mut self, source: &Self);          // Vec::clone_from: keeps the buffer
}
impl<I, T> Col<I, T> {
    pub(crate) fn reset_with(&mut self, len: usize, fill: impl FnMut() -> T);
    pub(crate) fn is_strictly_ascending(&self) -> bool where T: Ord;
}
```

- An index is built through `Idx::from_idx` alone, the range-checked route; the private field
  enforces it.
- `Col::clone_from` keeps its allocation; the derive's does not, so `RemainingOutputReads::seed`
  allocated a column each run.

#### Output types — `exec/error.rs`, `exec/compile/compiled_graph.rs`, `data/data_type/mod.rs`

```rust
// exec/error.rs
pub enum RunError {
    Invoke(InvokeError),
    /// A lambda wrote a value its output's declared type does not hold.
    OutputType { outputs: Vec<usize> },
    /* the rest unchanged */
}

// exec/compile/compiled_graph.rs
pub struct CompiledGraph {
    /* … */
    /// Each output's declared type, which a run holds what a lambda wrote to.
    pub(crate) output_types: Col<OutputIdx, DataType>,
}
// data/data_type/mod.rs
impl DataType { pub(crate) fn holds(self, value: &DynamicValue) -> bool; }
```

#### Document — `graph/`

```rust
// graph/scope.rs
impl<'a> Scope<'a> {
    pub fn decl(self, node: &Node) -> NodeDecl<'a>;        // was `Node::decl`
}
```

Batch 9 alone:

```rust
// graph/compound.rs
pub struct CompoundDecl { pub name: String, pub description: Option<String>, pub interface: Interface }
pub struct Compound { pub decl: CompoundDecl, pub graph: Graph }          // the travel value
/// A compound as a set holds it: what `GraphSet::compound` hands out.
#[derive(Clone, Copy, Debug)]
pub struct CompoundRef<'a> { pub decl: &'a CompoundDecl, pub graph: &'a Graph }
// graph/graph_set/mod.rs
pub struct GraphSet {
    main: Graph,
    decls: BTreeMap<CompoundId, CompoundDecl>,
    bodies: BTreeMap<CompoundId, Graph>,                                   // one key set, kept by insert/remove
    type_names: TypeNames,
}
impl GraphSet {
    pub fn compound(&self, id: CompoundId) -> Option<CompoundRef<'_>>;
    pub fn insert_compound(&mut self, id: CompoundId, compound: Compound);
    pub fn remove_compound(&mut self, id: CompoundId) -> Option<Compound>;
    /// One graph to change and the scope it resolves against, borrowed apart.
    pub(crate) fn graph_and_scope<'a>(&'a mut self, library: &'a Library, graph: GraphId)
        -> (&'a mut Graph, Scope<'a>);
}
// graph/scope.rs
pub struct Scope<'a> { library: &'a Library, decls: &'a BTreeMap<CompoundId, CompoundDecl>, owner: Option<&'a CompoundDecl> }
// graph/node_decl.rs
pub enum NodeDecl<'a> { Func(&'a Func), Instance(&'a CompoundDecl), Boundary { side: Boundary, owner: &'a CompoundDecl } }
// graph/load.rs — new
pub(crate) struct GraphLoad<'a> { graph: &'a mut Graph, scope: Scope<'a>, before: Before<'a>, repairs: &'a mut Vec<Repair> }
```

- `decls` and `bodies` hold one key set: `insert_compound` and `remove_compound` are the only
  writes of either, and each takes or gives a whole `Compound`.
- The file keeps one map of whole compounds, so a definition stays one value in a document, a
  clip and a library file: `GraphSet` writes through a borrowed repr of `CompoundRef`s and reads
  through `#[serde(try_from)]` a map of `Compound`s.
- A load step is one method of `GraphLoad`, reading under the scope and writing the graph in
  place; the steps keep the order `GraphSet::reconcile` runs them in today.

#### Declarations — `library/`, `graph/func/`, `graph/ports.rs`, `graph/value_domain/`, `data/`

```rust
// library/library_builder.rs — new
#[derive(Debug, Default)]
pub struct LibraryBuilder { /* the maps `Library` holds today, and each type's codec */ }
impl LibraryBuilder {
    pub fn add(&mut self, func: Func);
    pub fn add_enum(&mut self, id: EnumId, enum_type: EnumType);
    pub fn add_custom_type(&mut self, id: CustomTypeId, custom_type: CustomType);
    pub fn add_file_kind(&mut self, id: FileKindId, file_kind: FileKind);
    pub fn add_redirect(&mut self, redirect: FuncRedirect);
    pub fn merge(&mut self, other: Self);
    /// # Panics: what `Library::check` panics on today.
    pub fn build(self) -> Library;
}

// library/mod.rs — no mutators
#[derive(Debug, Default)]
pub struct Library {
    funcs: BTreeMap<FuncId, Func>,
    enums: BTreeMap<EnumId, EnumType>,
    custom_type_names: BTreeMap<CustomTypeId, String>,
    file_kinds: BTreeMap<FileKindId, FileKind>,
    /// Each ends at a registered func, its port map composed over the whole chain.
    redirects: BTreeMap<FuncId, FuncRedirect>,
    /// The custom and enum types by name, then id.
    retypable: Vec<DataType>,
    /// The one holder of each codec.
    codecs: Arc<Codecs>,
}
impl Library {
    pub fn data_types(&self) -> &[DataType];
    pub fn redirect(&self, from: FuncId) -> Option<&FuncRedirect>;
    pub fn default_const(&self, input: &InputDecl) -> Option<ConstValue>;   // replaces `DataType::default_value`
}

// graph/func/func_decl.rs — new
#[derive(Debug, Clone)]
pub(crate) struct FuncDecl {
    pub(crate) id: FuncId,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) sink: bool,
    pub(crate) default_cache_mode: CacheMode,
    pub(crate) behavior: FuncBehavior,
    pub(crate) description: Option<String>,
    pub(crate) ports: Ports,
    pub(crate) events: Vec<Numbered<EventDecl>>,
    /// One per event, in `events`' order; `FuncBuilder::event` pushes both.
    pub(crate) event_lambdas: Vec<EventLambda>,
    pub(crate) lambda: FuncLambda,
}
// graph/func/func_builder.rs
pub struct FuncBuilder { decl: FuncDecl }
impl FuncBuilder {
    pub fn checked(self) -> Result<Func, FuncValidationError>;
    pub fn build(self) -> Func;                    // sugar over `checked`, panics
}
// graph/func/mod.rs
pub struct Func { decl: FuncDecl, signature: FuncSignature }
impl Func {
    pub fn events(&self) -> &[Numbered<EventDecl>];
    pub(crate) fn event_lambda(&self, event_idx: usize) -> &EventLambda;
}
// graph/node_decl.rs
impl<'a> NodeDecl<'a> { pub fn events(self) -> &'a [Numbered<EventDecl>]; }

// graph/value_domain/value_variant.rs — moved from `graph/func/`
pub struct ValueVariant { pub label: String, pub value: ConstValue }

// graph/value_domain/{int_span,float_span}.rs
#[derive(Deserialize)] #[serde(try_from = "IntSpanRepr")]  pub struct IntSpan { /* … */ }
#[derive(Deserialize)] #[serde(try_from = "FloatSpanRepr")] pub struct FloatSpan { /* … */ }

// data/data_type/enum_type.rs
impl EnumType {
    /// # Panics: on an empty or a repeated variant, as `FileKind::new` on an extension.
    pub fn new(name: impl Into<String>, variants: impl IntoIterator<Item = impl Into<String>>) -> Self;
}

// exec/compile/compiled_graph.rs
impl CompiledGraph {
    /// Each output a node's inputs are wired to, in input order.
    pub(crate) fn wires(&self, node_idx: NodeIdx) -> impl Iterator<Item = OutputAddr> + '_;
}
// data/dynamic_value.rs
impl DynamicValue {
    pub fn into_string(self) -> Result<String, Self>;
    pub fn into_fs_paths(self) -> Result<Vec<String>, Self>;
}
```

- **A `Library` value is whole**: `build` is the one place the cross-table check runs, so no
  caller remembers `check()`, redirect chains are flattened once, and every walk of the funcs is
  in id order.
- **One field list** for a func, in `FuncDecl`; `FuncBuilder`'s fields are private, so a setter is
  the one route.
- **One shape for an event**, so `FuncEvent`, `NodeEvents`, `EventRef` and `ExactChain` go.
- **`ConstValue` has no `Display`**: `ValueText` is the round-trip text and the editor's
  `ShownValue` the one a person reads. `FloatSpan::contains` requires a finite value.

### Ownership graph

```
host thread
├─ owns Document (darkroom)
│  └─ owns GraphSet
│     ├─ owns main: Graph
│     │  ├─ owns nodes: BTreeMap<NodeId, Node>
│     │  │  └─ handle FuncId → Library.funcs · CompoundId → GraphSet.decls
│     │  ├─ owns wiring: Wiring
│     │  │  └─ handle InputPort / OutputPort / EventPort / TriggerPort → nodes
│     │  ├─ owns ghost_nodes: BTreeMap<NodeId, GhostNode>
│     │  └─ owns ghost_wiring: GhostWiring → nodes ∪ ghost_nodes
│     ├─ owns decls: BTreeMap<CompoundId, CompoundDecl>        (batch 9; `compounds` until then)
│     ├─ owns bodies: BTreeMap<CompoundId, Graph>
│     └─ owns type_names: TypeNames
├─ owns RuntimeLibrary (darkroom)
│  └─ owns Library                                              replaced whole, never edited
│     ├─ owns funcs: BTreeMap<FuncId, Func>
│     │  └─ shared FuncLambda, EventLambda                      (kept, below)
│     └─ shared codecs: Arc<Codecs>                             (kept)
├─ owns Compiler                                                scratch, cleared per compile
│  └─ borrows &GraphSet, &Library for one `compile`
│     └─ makes CompiledGraph → shared Arc<CompiledGraph>        (kept)
└─ owns Worker
   ├─ owns AbortOnDropHandle<()> → worker task
   └─ shared Arc<Mailbox>                                       (kept)
      ════ thread boundary: host → worker ════
      crosses under one lock: WorkerMessage (Arc<CompiledGraph>, DiskStore, Vec<ProgramId>),
      ProgramEvent, cancel, shutdown
      bound: one graph op, id lists bounded by the program, one entry per event

worker task (one tokio task)
└─ owns WorkerTask
   ├─ shared Arc<Mailbox>
   ├─ owns callback: Box<dyn Fn(WorkerReport) + Send + Sync>
   │     ════ thread boundary: worker → host, called in place ════
   │     crosses: WorkerReport (Arc<CompiledGraph> echo, Arc<RunSummary>, progress)
   ├─ owns intent: BatchIntent                                  swapped with the mailbox's
   ├─ owns summary: RunSummaryPublisher → shared Arc<RunSummary>  (kept)
   ├─ owns outcome: ExecOutcome
   ├─ owns epoch: RunEpoch                                      watch<u64>, sender here
   ├─ owns event_loop: Option<ActiveEventLoop>
   │  └─ owns tasks: JoinSet<Infallible>
   └─ owns engine: ExecEngine
      ├─ shared compiled: Arc<CompiledGraph>
      ├─ owns planner: Planner                                  DFS scratch
      ├─ owns schedule: RunSchedule                             columns by NodeIdx / OutputIdx
      ├─ owns cache: RuntimeCache
      │  ├─ owns slots: Col<NodeIdx, RuntimeSlot>
      │  │  └─ owns OutputSnapshot → shared DynamicValue::Custom(Arc<dyn CustomValue>)  (kept)
      │  ├─ owns disk_store: DiskStore
      │  └─ owns paths: PathIdentities
      │     └─ owns job: StampJob   ════ moves to the blocking pool and back, once per pass ════
      └─ owns executor: Executor
         ├─ owns ctx_manager: CtxManager → CancelToken (the mailbox's, cloned per run)
         ├─ owns scratch: Vec<DynamicValue>                     empty between turns
         ├─ owns remaining_reads, outcomes                      per-run columns
         └─ owns nodes: Col<NodeIdx, NodeRuntime>
            └─ shared event_state: SharedAnyState               (kept)

event task (one tokio task per armed event)
├─ owns EventFires                                              the lambda's stream
├─ shared SharedAnyState
├─ shared Arc<Mailbox>                                          fires through it
└─ owns watch::Receiver<u64> of the epoch

blocking pool (one closure per blob operation, per stamp pass)
└─ receives by value: BlobTarget, Arc<Codecs>, the demand or the values; returns the result
```

Every shared-ownership case of the current code, with its verdict:

| Case | Where | Verdict |
|---|---|---|
| `Arc<CompiledGraph>` | `worker/protocol.rs:16,40`, `exec/engine/mod.rs:40` | Kept: immutable, held by the host, the mailbox and the engine across threads; the echo in `Installed` tells the host which program reports describe. |
| `Arc<Codecs>` with `Arc::make_mut` | `library/mod.rs:54,139`, `compiled_graph.rs:120` | Kept: a program on the worker outlives the library it was compiled from. With `LibraryBuilder`, the table is built once and `make_mut` goes. |
| `Arc<dyn CustomValueCodec>` in `CustomType.codec` | `data/data_type/custom_type.rs:18` | Kept as what a caller hands the builder; replaced inside `Library`, which held each codec twice: `Codecs` is the one holder. |
| `DynamicValue::Custom(Arc<dyn CustomValue>)` | `data/dynamic_value.rs:38` | Kept: an immutable value too costly to copy, held by a slot, each consumer and a twin at once. `into_custom` moves on the last holder. |
| `FuncLambda(Arc<dyn AsyncLambdaFn>)` | `graph/func/lambda.rs:64` | Kept: the library on the host and each program on the worker run it. |
| `EventLambda(Arc<dyn Fn>)` over an inner `Arc<F>` | `graph/func/event.rs:14,35-37` | Kept: the stream needs an owned lambda; a single `Arc` through an `Arc<Self>` receiver would save one allocation per declared event, which is nothing. |
| `Arc<Mailbox>`, `Mutex<Pending>` | `worker/mod.rs`, `worker/mailbox/mod.rs` | Kept: it is the channel, held by the handle, the task and each event task. |
| `Arc<RunSummary>` and its `Arc::get_mut` reuse | `worker/run_summary/mod.rs:30,43-47` | Kept: its "allocates nothing" holds only while the host has dropped the last summary, as its doc says. |
| `SharedAnyState { Arc<Mutex<AnyState>> }` | `runtime/shared_any_state.rs:10` | Kept: a func lambda in a run and an event lambda between runs both write it. Its guard is `!Send`, so holding it across an await does not compile. |
| `ram_seen: HashSet<usize>` of `Arc::as_ptr` | `runtime_cache/mod.rs:67,210` | Kept: every `Arc` is alive for the pass, so no address repeats. Spelled `Arc::as_ptr(arc).addr()`. |
| `LazyLock<Func>` | `graph/node/special/run_sinks.rs:24` | Kept: immutable, built once. |

No serialized type holds an `Arc` or `Rc`.

### Call graph

```
Worker::send(&self, WorkerMessage) -> Result<(), WorkerExited>          worker/mod.rs
└─ Mailbox::post(&self, msgs)                                           worker/mailbox.rs
   └─ BatchIntent::push(&mut self, WorkerMessage)                       worker/batch_intent

Worker::cancel(&self)
└─ Mailbox::cancel(&self)
   └─ BatchIntent::void_runs(&mut self)

Worker::exit(&mut self, Duration) -> Result<(), ExitError>
├─ Mailbox::shut_down(&self)
├─ Mailbox::cancel(&self)
└─ tokio::time::timeout over the task handle                            aborts the task at the deadline

event task (per armed event)
├─ EventFires::next(&mut self)                                          decl/func/event_lambda.rs
├─ Mailbox::fire(&self, ProgramEvent)
│  └─ BatchIntent::push_event(&mut self, ProgramEvent)
└─ watch::Receiver<u64>::changed()                                      parks until a batch ends

WorkerTask::run(self)                                                   worker/worker_task
├─ select: Mailbox::posted(&self) | ActiveEventLoop::failed(&mut self) -> Infallible
├─ Mailbox::take_into(&self, &mut BatchIntent) -> Taken
├─ WorkerTask::apply_intent(&mut self)
│  ├─ BatchIntent::void_events(&mut self)                               when the loop stops or rebuilds
│  ├─ ActiveEventLoop::stop(&mut self)
│  ├─ Mailbox::void_events(&self)
│  ├─ ExecEngine::install(&mut self, Arc<CompiledGraph>)                exec/engine
│  │  ├─ RuntimeCache::reconcile(&mut self, prev, program)
│  │  └─ Executor::reconcile(&mut self, prev, program)
│  ├─ ExecEngine::set_disk_store(&mut self, DiskStore)
│  ├─ ExecEngine::prune_store(&mut self) -> Vec<RemovalError>           once a store and a program are both present
│  │  └─ DiskStore::prune(&self, live ids) -> Vec<RemovalError>                [I/O]
│  ├─ ExecEngine::evict_cache(&mut self, ids) -> Vec<CacheNodeFailure>
│  │  └─ DiskStore::remove_nodes(&self, Vec<ProgramId>) -> Vec<Result<(), RemovalError>>  [I/O]
│  ├─ ExecEngine::flush_cache(&mut self, ids) -> CacheFlushReport              [I/O]
│  ├─ WorkerTask::execute(&mut self)                                    borrows &self.intent.seeds
│  │  ├─ ExecEngine::execute(&mut self, ExecuteRequest, &mut ExecOutcome) -> ExecResult<()>
│  │  └─ ActiveEventLoop::start(Vec<ArmedEvent>, &Arc<Mailbox>, &RunEpoch) -> Self
│  └─ RunEpoch::advance(&self)
└─ Mailbox::close(&self)

ExecEngine::execute(&mut self, ExecuteRequest, &mut ExecOutcome) -> ExecResult<()>
├─ Planner::plan(&mut self, &CompiledGraph, &RunSeeds, &mut RunSchedule) -> ExecResult<()>
├─ RuntimeCache::prepare(&mut self, &CompiledGraph, nodes, CancelToken)
│  └─ StampJob::run(self) -> Self                                              [I/O] one blocking pass
├─ resolve::sweep(&CompiledGraph, &mut RunSchedule, &mut RuntimeCache)
│  ├─ RuntimeCache::stamp_digests(&mut self, &CompiledGraph, &RunSchedule)
│  ├─ RuntimeCache::mark_leaders(&mut self, &CompiledGraph, &RunSchedule)
│  ├─ RuntimeCache::probe_reuse(&mut self, &CompiledGraph, NodeIdx, &[OutputDemand]) -> bool
│  │  └─ BlobTarget::covers_demand(self, Arc<Codecs>, demand) -> bool          [I/O] from `Unknown`, and `Holds` under the stamp
│  └─ RuntimeCache::twin_leader(&self, NodeIdx) -> Option<NodeIdx>
├─ Executor::run(&mut self, RunRequest, &mut ExecOutcome)
│  └─ per node of `process_order`
│     ├─ task::yield_now()                                              before `Run | Reuse | Twin` only
│     ├─ ExecFrame::run_node(&mut self, NodeIdx)
│     │  ├─ ExecFrame::serve_reuse → RuntimeCache::hydrate_reuse -> ReuseOutcome
│     │  │  └─ BlobTarget::read(self, Arc<Codecs>, demand) -> Option<OutputSnapshot>  [I/O]
│     │  ├─ ExecFrame::serve_twin
│     │  └─ ExecFrame::invoke_node
│     │     ├─ ExecFrame::collect_inputs(&mut self, NodeIdx)
│     │     ├─ RuntimeSlot::invoke_outputs(&mut self, &[OutputDemand]) -> &mut [DynamicValue]
│     │     ├─ FuncLambda::invoke(&self, Invocation) -> InvokeResult<()>       one boxed future
│     │     ├─ Vec::clear on `inputs`
│     │     ├─ DataType::holds(self, &DynamicValue) -> bool             per demanded output
│     │     └─ RuntimeCache::store_node(&mut self, …) -> StoreOutcome
│     │        └─ BlobTarget::store(&self, Arc<Codecs>, Vec<DynamicValue>, StorePolicy) -> StoreResult  [I/O]
│     └─ ExecFrame::relieve(&mut self, u64, MemoryProbe)
│        └─ RuntimeCache::relieve(&mut self, &CompiledGraph, u64, impl Fn(NodeIdx, &RuntimeSlot) -> bool) -> u64
├─ RuntimeCache::release_dead_outputs(&mut self, &CompiledGraph)
├─ RuntimeCache::measure_resident_ram(&mut self) -> RamUsage
└─ Executor::collect_outcome(&mut self, CollectRequest, &mut ExecOutcome)

Compiler::compile(&mut self, &GraphSet, &Library) -> CompiledGraph      exec/compile
├─ GraphSet::validate_with(&self, &Library) -> Result<(), GraphSetValidationError>   asserted
├─ Compiler::place_nodes / walk / lower                                 fills the columns
└─ CompiledGraph::validate_debug(&self)

GraphSet::reconcile(&mut self, &Library, &Manifest) -> GraphSetReconciled   graph/graph_set
├─ Graph::repair_structure(&mut self, &mut Vec<Repair>)
├─ dfs::back_edges(compounds, placed) -> BackEdges<CompoundId, NodeAddr>
├─ Graph::detach_into_ghosts(&mut self, Vec<Detaching>, &mut Vec<Repair>)   one pass of each table
└─ per graph: the read-then-write steps as today (one `GraphLoad` pass after batch 9)

GraphSet::digest(&self, CompoundId) -> CompoundDigest                   graph/graph_set/digest.rs
└─ dfs::back_edges([id], placed) -> BackEdges<CompoundId, ()>           then one hash per compound, post-order

Graph::admit_bind(&self, Scope, InputPort, &Binding) -> Result<(), BindRejection>   graph/mod.rs
├─ Graph::admit(&self, Scope, InputPort, &Binding) -> Result<(), BindingRejection>
└─ dfs::reaches(consumer, producer, wires_into) -> bool

LibraryBuilder::build(self) -> Library                                  library/library_builder.rs
└─ FuncRedirect chains composed, `retypable` sorted, `Codecs` collected
```

Every edge points from the host and the worker loop down to the columns; `exec` never names
`worker`, and `graph` never names `exec`.

### Module layout

```
containers/   col, idx_set, unique
data/         const_value, dynamic_value, codec, ram_usage, path_form
              data_type/{mod, custom_type, enum_type (+ EnumVariants), file_kind, type_ref (TypeRef, from graph/type_names)}
identity/     mod (ids, port addresses, from graph/identity), inline_ids, instance_path, program_id
runtime/      any_state, shared_any_state, log_entry, ctx_manager
decl/         cache_mode (CacheMode, Keep, from graph/node)
              ports (Ports, Numbered, InputDecl, OutputDecl, EventDecl), ports/error (PortsError)
              value_domain/{mod, int_span, float_span, span_error, value_variant}
              func/{mod, func_decl, func_builder, func_lambda, event_lambda, func_signature (+ FuncRef), error, macros}
library/      mod (Library), library_builder, func_redirect
graph/        mod (Graph), wiring, ghost_wiring, ghost_node, detached, dfs, node/, node_decl, node_ref,
              binding, subscription, scope, compound, interface, interface_change, port_map/, manifest,
              saved_decl, reconciled, type_names (TypeNames), serde/, error, load (batch 9)
              graph_set/{mod, digest, program_shape, unknown_type}
exec/         as today, with worker-free `report/`
worker/       mod, mailbox, batch_intent/, worker_task/, active_event_loop/, run_epoch, run_summary/,
              protocol, activity, error
builtin/      as today
```

```
containers ← data ← identity ← runtime ← decl ← library ← graph ← exec ← worker
                                                    ↖ builtin (decl, library, runtime)
```

No arrow returns, with one exception kept: `identity`'s aliases name `Node`, `Func` and `Compound`
as the tag of `common::Id<T>`, a `PhantomData<fn() -> T>` that reads no state of them. A marker
type per id would cut it and add a name for each. The cycles the order cuts, each by the edge
against the layering:

- `graph ⇄ library` — the declaration types `library` needs move to `decl`; `BindingRejection`'s
  wire cases move beside `Library::admit_wire`; `FuncRedirect`'s `NodeRemap` stays in
  `graph::port_map` and the redirect holds the port map it needs as `decl` data.
- `graph ⇄ runtime` — `CtxManager` and `LogEntry` name `ProgramId`, which moves to `identity`,
  under `runtime`; the lambdas that take a `CtxManager` sit above it, in `decl`. `FuncRef` leaves
  `identity` for `decl/func/func_signature.rs`, so `identity` imports no signature.
- `graph::func ⇄ graph::node` — `CacheMode` moves to `decl`.
- `ports → func::value_variant`, `value_domain → func::value_variant` — `ValueVariant` moves under
  `value_domain`; `Ports::default_bindings` moves to the graph side.
- `node ⇄ scope` — `Node::decl(scope)` becomes `Scope::decl(&Node)`.
- `scope ⇄ graph_set` — `Scope` holds the compound table, not the set.
- `graph (mod) ⇄ compound` — `add_func_node` and `add_instance`, sugar over
  `Graph::insert_seeded(node_id, node, &Ports)`, leave the graph.
- `error ⇄ graph_set` — `ProgramShape` moves to `graph_set/program_shape.rs`.
- `instance_path ⇄ program_id` — both sit on `InlineIds`.
- inside `data` — `EnumVariants` moves to `enum_type.rs`; `TypeRef` to `data_type/type_ref.rs`.

`graph/mod.rs` keeps the struct, the node and ghost-node tables, `input_state` /
`subscription_state` and their setters, `admit*`, `validate*`, `shown_*`, snapshot, detach and
attach. Out of it: `wiring.rs`, `dfs.rs`, and after batch 9 `load.rs` with the reconcile steps and
their satellites (`Before`, `GhostNotes`, `AdmittedGhosts`). Its module doc's "every method a
`Graph` has, in one impl" is dropped, as five other files already hold an `impl Graph` or
`impl GraphSet`.

### Agreement

Every type a graph names is declared above or kept; `Worker` owns a mailbox and a task handle and
nothing else, as its tree shows; the event task's two shared edges are `Arc<Mailbox>` and
`SharedAnyState`, both in the table; `GraphLoad` borrows one graph mutably and a
`Scope` over `decls`, two fields `graph_and_scope` splits; `Worker::exit` returns the `ExitError`
`worker/error.rs` declares; the predicate `ExecFrame::relieve` hands `RuntimeCache::relieve`
reads `node_outcomes` and `schedule.states`, which the frame borrows for the run, and the slot the
cache passes it; no borrow in the call graph outlives `ExecEngine::execute`.

## Decisions

- **An index's field is private.** rustc's `newtype_index!` "creates a struct type `S` that can be
  used as an index with `IndexVec`" behind accessors, and cranelift's `entity_impl!` is "the common
  implementation of a 32-bit entity reference"
  ([`newtype_index`](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_index/macro.newtype_index.html),
  [cranelift-entity](https://docs.rs/cranelift-entity/latest/cranelift_entity/)). Not taken: a
  niche for `Option<NodeIdx>`, whose one column `Twin { leader }` removes.
- **Not taken: a pass-through hasher for the twin leaders' map**, as rustc's `Unhasher` for keys
  "that already have hash-like quality"
  ([`unhash`](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_data_structures/unhash/index.html)):
  the map moved to hashbrown, whose default hasher is no SipHash, and one insert a stamped node a
  run is no cost a custom `Hasher` repays.
- **Not taken: a disk belief that outlives the value.** It would save a probe for a node whose
  digest changed, which then runs anyway, and keep a header for the load. A failed write, the one state that must outlive the value, is the slot's
  `BlobState::Failed`.
- **A belief about disk saves a read only where being wrong costs a recompute.** Refused: trusting
  `Holds` under the stamp with no probe at the sweep, which saves one read per reused frontier
  node per run and turns a blob deleted behind the engine into a node that fails with
  `CacheLoadFailed`, its producers already cut.
- **A custom type's identity stays its name and its codec instance** (`custom_type.rs:47-54`). No
  production path registers one type from two instances: `lens` registers `Image` once. Refused:
  identity by the codec's `version()`, which two different codecs share.
- **A func's events are two columns in one order**, `events` and `event_lambdas`, pushed together
  by `FuncBuilder::event`, their equal length asserted in `checked`. Refused: one
  `Vec<FuncEvent>`, from which no `&[Numbered<EventDecl>]` can be borrowed, the reason
  `NodeEvents`, `EventRef` and `ExactChain` exist.
- **Not taken: one test a behaviour for `a_manifest_tells_a_load_what_each_func_declared` and
  `a_subscription_lands_on_a_declared_trigger_and_an_interface_edit_refuses_it`.** The second is
  one story whose steps build on each other's state, and the first's phases share its fixture's
  closures; split, each would carry the fixture again for a better failure name alone.
- **`Library` is built, then read.** "Builders enable construction of complex values"
  ([C-BUILDER](https://rust-lang.github.io/api-guidelines/type-safety.html)); `FuncBuilder` →
  `Func` is the same shape one level down. Refused: `check()` as a call the composer remembers.
- **`IntSpan` and `FloatSpan` decode through `#[serde(try_from)]`**: "deserialize this type by
  deserializing into `FromType`, then converting fallibly"
  ([container attributes](https://serde.rs/container-attrs.html)), as `ProgramId` already does.
- **`Numbered<T>` keeps its `Deref`.** "Only smart pointers implement `Deref` and `DerefMut`"
  ([C-DEREF](https://rust-lang.github.io/api-guidelines/predictability.html)); the departure is
  that a numbered port reads as its port at every site, and the alternative is `.port.` at each.
- **Not taken: a batch detach for edits and redirects (`detach_nodes`).** A redirect's remap
  moves one node's ends, so a wire between two redirected nodes needs the first move before the
  second, which a batch splits across records; an edit removes one node a step. The load's
  ghosting of every refused node, the many-node case, takes the wiring in one pass of each table.
- **Not taken: a per-revision cache of a node's ghost ends in the editor.** The ghost tables are
  empty on a healthy document, so the scan costs nothing there; `GhostWiring::wires_from` and
  `subscriptions_to` name the two reverse queries a cache would keep.
- **Not taken: `RefusedWiring` in `AdmittedGhosts`' two-list shape.** Each refusal carries its
  rejection, which the enum keeps beside its key; two lists of pairs would only move the match.
- **Not taken: a caller's scratch for the loop check.** A wire drag asks it for the one or two
  ports under the pointer a frame, and each answer is a walk of the wires into one node.
- **Not taken: the flattened subscriber count, asserted.** A trigger that crosses a boundary fans
  out inside and a disabled subscriber drops, so only the walk knows the count; the authored
  subscriptions stay a capacity hint, and the compile's doc names the pool as the one that can
  grow.
- **Not taken: the probe's header kept for the load (`RuntimeCache.probed`).** The load reads
  the header again inside its one blocking closure: a small read the page cache serves, no hop of
  its own, and it holds the file as it is at the load, where a kept header would meet a blob
  replaced since the probe as a hash failure and delete it.
- **The store's bound is a prune, `DiskStore::prune`, not a sweep**, as the worker's flush of
  every node and the resolver's pass already take that word. A temporary is the store's by the
  name `common::fs::temporary_destination` reads, so a file of any other name stays.
- **A set holds a compound's declaration apart from its body.** A scope reads a compound's name,
  description and interface and never its graph, so a graph of the set can change while a scope
  lives, and a load step reads and writes in one method. It amends "a repair reads, then writes":
  still refused, taking a graph out of its set for the call, which leaves the set wrong while a
  scope reads it. As MaterialX, where a `nodedef` declares the ports and an implementation stands
  behind it ([node definitions](https://kwokcb.github.io/MaterialX_Learn/documents/node_definitions.html)),
  the source `node_decl.rs` cites. Refused: read-then-write kept and only moved to its own file,
  which keeps seven hand-off types and a clone of every graph for each interface edit.
- **Not taken: views `Planned` / `Resolved` over `RunSchedule`.** The typestate pattern "encodes
  information about an object's run-time state in its compile-time type"
  ([typestate](https://cliffle.com/blog/rust-typestate/)); the three passes are three consecutive
  statements at one production site, and the views would cost every fixture a constructor.
- **Not taken: a remap column for `install`.** The merge walk runs twice over a sorted id column,
  microseconds per install, and a map would allocate.
- **Not taken: a hot and cold split of `ExecNode`, constants in a side column.** At the 65 536-node
  bound the two columns are about 12 MiB and no pass is bound by them.
- **Not taken: `custom_type_names` in `Library`, its codecs held by `Codecs` alone.** A custom
  type's identity is its name and its codec instance (`CustomType::same_as`), so the library
  keeps the `CustomType` whole and the codec table holds a second `Arc` of each codec, built once
  at the build. A test's in-place edit (`internals`) takes the library back to a builder, which
  needs the type whole.
- **A test edits a built library in place, under `internals`.** `Library::add` and the type
  registrations there take the library back to a `LibraryBuilder`, edit it and assemble it
  again, unchecked: a fixture grows the library its graph resolves against as a test goes, and a
  test whose subject is the check builds through `LibraryBuilder::build`. A redirect has no
  in-place edit, as the library holds only the flattened chains.

- **Batch 8 as built departs from the layout in four places.** `BindingRejection` moves whole
  to `decl/ports/error.rs`, as every case is a port's or a type's, rather than splitting its wire
  cases out. `SavedDecl` and the port maps move to `decl` too (`decl/saved_decl.rs`,
  `decl/port_map.rs`), so `FuncRedirect` names no graph type, and the move of a node's wiring
  through a remap stays in the graph as `graph/remap/`. `add_func_node` and `add_instance`, which
  only tests call, move under `internals`. `ProgramShape::of_graph` keeps its `&Graph`: the
  `error → program_shape → graph` loop it leaves runs through the parent module, as each child
  does. `GraphSet::unknown_types` stays a method of the set, beside the types in
  `unknown_type.rs`.

# Part 2 — Plan

Each batch leaves the build and the tests green. Whoever addresses an item deletes it.

## Batch 9 — a compound's declaration apart from its body

Files: `graph/compound.rs`, `graph/graph_set/`, `graph/scope.rs`, `graph/node_decl.rs`,
`graph/load.rs` (new), `graph/mod.rs`, `graph/interface_change.rs`, `graph/port_map/`, the
`darkroom` callers of `GraphSet::compound`. After batch 8.

- [ ] `graph/scope.rs:18-23` — `Scope` borrows the whole set to read a compound's name,
  description and interface, so no graph of it can change while a scope lives. Target:
  `CompoundDecl`, `GraphSet { decls, bodies }`, `Scope` over `decls`.
- [ ] `graph/mod.rs:210-287,1071-1707` — eight read and write pairs, `reconciled: &mut Reconciled`
  through twelve methods, and the hand-off types `Detaching`, `Refusal`, `AdmittedGhosts`,
  `RefusedWiring`, `GhostNotes`, `BindingNote`, `SubscriptionNote`. Target: `GraphLoad`, one
  method a step, in `graph/load.rs`.
- [ ] `interface_change.rs:177` — `plan_interface` clones every graph of the set to change one
  interface. Target: a clone of `decls`.
- [ ] `port_map/mod.rs:666` — `replaced` builds a `Reconciled` it discards. Target: the pass's
  repairs, read or dropped by the caller.
- [ ] The module doc's "a repair reads, then writes" — amended to the decision taken, with the
  refused alternative kept.
