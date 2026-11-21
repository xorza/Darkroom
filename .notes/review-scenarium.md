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
  columns reset in place. They need tightening only (batch 7).
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
  on. Only the holes in what they check are findings (batch 7).

## Root causes

3. **Blob I/O is asynchronous one operation at a time.** `tokio::fs::File` turns each read, write,
   seek and stat into its own blocking-pool round trip, the format seeks back to patch each
   output's length, and the probe's header read is thrown away and repeated at the load.
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

// exec/cache/runtime_cache/mod.rs
pub(crate) struct RuntimeCache {
    /* … */
    /// The header each reused node's probe read this run, which its load takes.
    probed: Col<NodeIdx, Option<BlobHeader>>,
}
```

#### Blob I/O — `data/codec/mod.rs`, `exec/cache/disk_store/`, `exec/cache/stamp_job/mod.rs`

```rust
// data/codec/mod.rs — no `async_trait`, no tokio
pub trait CustomValueCodec: Send + Sync + Debug {
    fn version(&self) -> u32;
    fn encode(&self, value: &dyn CustomValue, writer: &mut dyn io::Write) -> Result<(), CodecError>;
    fn decode(&self, reader: &mut dyn io::Read, byte_len: u64) -> Result<Arc<dyn CustomValue>, CodecError>;
}

// exec/cache/disk_store/format/mod.rs — synchronous; `FORMAT_VERSION` 12, `DESC_LEN` 64
struct OutputDesc {
    kind: OutputKind,
    payload_len: u64,
    /// blake3 of the payload's bytes; of no bytes for an unbound output.
    payload_hash: [u8; 32],
}
pub(crate) struct BlobHeader { digest: Digest, descs: Box<[OutputDesc]> }
impl BlobHeader {
    pub(crate) fn covers_demand(&self, codecs: &Codecs, demand: &[OutputDemand]) -> bool;
    pub(crate) const fn digest(&self) -> Digest;
}
pub(super) struct BlobWriter<'a, W: io::Write + io::Seek> { /* … */ }
pub(super) struct BlobReader<'a, R: io::Read> { /* … */ }
/// What a codec writes through: counts and hashes one payload, so its length and hash are never
/// read back from the stream.
struct PayloadWriter<'a, W: io::Write> { writer: &'a mut W, written: u64, hasher: blake3::Hasher }
/// What a codec reads through: at most the payload's length, hashed as it is read.
struct PayloadReader<'a, R: io::Read> { reader: io::Take<&'a mut R>, hasher: blake3::Hasher }

// exec/cache/disk_store/error.rs
pub enum FrameError {
    /* … */
    /// A payload's bytes do not hash to what its descriptor states.
    PayloadHash { output_idx: usize },
}

// exec/cache/disk_store/blob_target/mod.rs — each body one `common::task::run_blocking`
impl BlobTarget {
    pub(crate) async fn probe(&self) -> Option<BlobHeader>;                                  // [I/O]
    pub(crate) async fn read(&self, codecs: Arc<Codecs>, header: BlobHeader,
                             demand: Box<[OutputDemand]>) -> Option<OutputSnapshot>;         // [I/O]
    pub(crate) async fn store(&self, codecs: Arc<Codecs>, values: Vec<DynamicValue>,
                              policy: StorePolicy) -> StoreResult;                           // [I/O]
}
impl DiskStore {
    /// Remove these nodes' blobs in one blocking pass.
    pub(crate) async fn remove_nodes(&self, program_ids: Vec<ProgramId>) -> Vec<RemovalError>;  // [I/O]
    /// Remove each blob no node of `program` names, and each temp file last written over
    /// `STALE_TEMP` ago. One `read_dir`, in one blocking pass.
    pub(crate) async fn sweep(&self, program: Arc<CompiledGraph>) -> Vec<RemovalError>;          // [I/O]
}
/// A day: longer than any one blob takes to write, so a temp file this old is one a killed
/// process left, never a write in progress.
const STALE_TEMP: Duration = Duration::from_secs(24 * 60 * 60);

// exec/engine/mod.rs
pub(crate) struct ExecEngine {
    /* … */
    /// The attached store has not been swept against a program yet.
    sweep_owed: bool,
}
impl ExecEngine {
    /// Sweep the store if it is owed and a program is installed; else nothing.
    pub(crate) async fn sweep_store(&mut self) -> Vec<RemovalError>;                             // [I/O]
}

// exec/cache/stamp_job/mod.rs
pub(super) struct StampJob {
    requests: HashSet<String>,
    stamped: Vec<(String, FsPathId)>,
    /// A walked folder's relative paths, back to back.
    names: Vec<u8>,
    files: Vec<WalkedFile>,
    pending: Vec<PathBuf>,
}
struct WalkedFile { name: Range<u32>, identity: FileIdentity }

// common/src/fs/mod.rs
pub fn entry_metadata(entry: &DirEntry) -> io::Result<Metadata>;
```

- The header's length is fixed by the output count (`FIXED_LEN + count × DESC_LEN`), so the writer
  reserves it, streams each value once through a counter, then seeks to 0 and writes the finished
  header, each descriptor with the length and hash its `PayloadWriter` counted: one seek a blob,
  where each output cost a seek back, an 8-byte patch and a seek forward, and each seek a flush.
- **A payload is held to its hash.** After a codec decodes an output, the reader drains what the
  codec left of the payload, then compares the hash; a mismatch is `FrameError::PayloadHash`, a
  miss, and the blob is removed as an undecodable one is. The value is dropped, never served.
- A probe is one read of the fixed prefix and the descriptors. The header it read is kept for the
  run in `RuntimeCache.probed`, so the load reads the body alone.
- A directory stamp is one pass: each entry's metadata is the listing's own, and names sit in one
  buffer.
- **The store's bound is one blob per node of the installed program.** A store is swept once,
  when it and a program are both present: at `set_disk_store` with a program installed, else at
  the next `install`. Never against the empty program, which names no blob and would empty the
  folder. What it could not remove is reported as an eviction's failures are.

#### Document — `graph/`

```rust
// graph/wiring.rs — new
/// The bindings and subscriptions a graph holds; `GhostWiring`'s live counterpart, with its
/// method names.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Wiring {
    bindings: BTreeMap<InputPort, Binding>,
    subscriptions: BTreeSet<Subscription>,
}
impl Wiring {
    pub fn is_empty(&self) -> bool;
    pub fn binding(&self, port: InputPort) -> Option<&Binding>;
    pub fn bindings(&self) -> impl ExactSizeIterator<Item = (InputPort, &Binding)>;
    pub fn bindings_on(&self, node_id: NodeId) -> impl Iterator<Item = (InputPort, &Binding)>;
    pub fn wires(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_;
    pub fn subscriptions(&self) -> impl ExactSizeIterator<Item = Subscription> + '_;
    pub fn subscriptions_from(&self, emitter: NodeId) -> impl Iterator<Item = Subscription> + '_;
    /// The wiring that touches any of `nodes`, ascending and each once: one node is `&[id]`.
    pub fn touching(&self, nodes: &[NodeId]) -> Self;
    pub(crate) fn remove_touching(&mut self, nodes: &[NodeId]) -> Self;
}

// graph/ghost_wiring.rs — the same names, plus the two reverse queries the editor scans for
impl GhostWiring {
    pub fn wires_from(&self, producer: NodeId) -> impl Iterator<Item = (InputPort, &GhostBinding)>;
    pub fn subscriptions_to(&self, subscriber: NodeId) -> impl Iterator<Item = (Subscription, &GhostSubscription)>;
    pub fn touching(&self, nodes: &[NodeId]) -> Self;
    pub(crate) fn remove_touching(&mut self, nodes: &[NodeId]) -> Self;
}

// graph/mod.rs
pub struct Graph {
    nodes: BTreeMap<NodeId, Node>,
    wiring: Wiring,
    ghost_nodes: BTreeMap<NodeId, GhostNode>,
    ghost_wiring: GhostWiring,
}
impl Graph {
    pub const fn wiring(&self) -> &Wiring;
    /// Whether this graph may hold `binding` on `port`: both nodes live, the scope admits it, and
    /// it closes no loop. The rule the edit gate, a wire drag and a ghost's restore share.
    pub fn admit_bind(&self, scope: Scope<'_>, port: InputPort, binding: &Binding) -> Result<(), BindRejection>;
    pub fn placed(&self) -> impl Iterator<Item = (NodeId, CompoundId)> + '_;
    pub fn boundary_nodes(&self, side: Boundary) -> impl Iterator<Item = NodeId> + '_;
    /// Take `nodes` out, ascending and each once, with the wiring that touches each: one pass of
    /// each table. A wire between two of them goes with the lower id.
    pub fn detach_nodes(&mut self, nodes: &[NodeId]) -> Vec<DetachedNode>;
    /// Add `node` with each default its declaration's `ports` state; `add_func_node` and
    /// `add_instance` are sugar over it.
    pub fn add_seeded(&mut self, node: Node, ports: &Ports) -> NodeId;
}

// graph/error.rs — `GhostRejection` renamed, its cases unchanged: it answers a new bind too
pub enum BindRejection { GhostNode { node_id: NodeId }, Binding(#[from] BindingRejection), Cycle }

// graph/detached.rs
pub struct DetachedNode { node_id: NodeId, node: Node, wiring: Wiring, ghost_wiring: GhostWiring }

// graph/inline_ids.rs — new, private to `graph`
#[derive(Clone, Copy)]
pub(crate) struct InlineIds<const N: usize> { len: u8, ids: [NodeId; N] }
// graph/instance_path.rs:  pub struct InstancePath(InlineIds<7>);
// graph/program_id.rs:     pub struct ProgramId(InlineIds<8>);   // at least one id

// graph/interface_change.rs
pub struct InterfaceChange {
    pub id: CompoundId,
    pub before: Interface,
    pub after: Interface,
    pub fate: RefusedFate,
    pub graphs: BTreeMap<GraphId, WiringChange>,
}
pub struct WiringChange { pub refused: GhostWiring, pub named_before: GhostWiring, pub named_after: GhostWiring }

// graph/reconciled.rs
pub struct GraphSetReconciled { pub repairs: BTreeMap<GraphId, Vec<Repair>> }

// graph/node/mod.rs
pub struct Node {
    pub(crate) kind: NodeKind,
    /// `None` shows the declaration's name.
    pub name: Option<String>,
    pub disabled: bool,
}
// graph/scope.rs
impl<'a> Scope<'a> {
    pub fn decl(self, node: &Node) -> NodeDecl<'a>;        // was `Node::decl`
    /// The node's own name, else its declaration's.
    pub fn title(self, node: &'a Node) -> &'a str;
}
// graph/ghost_node.rs
impl GhostNode {
    /// The node's own name, else the name its saved declaration holds, else empty.
    pub fn title(&self) -> &str;
}
```

- **A node with no name of its own shows its declaration's**, a func node as an instance:
  `add_func_node` and `add_instance` both leave `name` `None`, and a rename to the empty string
  stores `None`. The empty-string sentinel and darkroom's `title` (`core/document/mod.rs:780-794`)
  go.

- **`Wiring`** — a port is bound once by construction, so `DetachedNode::new` loses its sort,
  its duplicate scan and two error variants; `bindings` is no longer a public field, so
  `set_input_state` is the one write and the live and ghost keys stay disjoint by it.
- **Gone:** `BindingEntry` (a `(InputPort, Binding)` pair of a `Wiring`), `RefusedBinding`,
  `RefusedSubscription`, `NamedGhost`, `RefusedWiring` (it takes `AdmittedGhosts`' two-list shape),
  `ExactChain`, `Graph::is_subscribed`, `reads_input` / `reads_output`.

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
└─ receives by value: BlobTarget, Arc<Codecs>, Vec<DynamicValue> or BlobHeader; returns the result
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
│  ├─ ExecEngine::sweep_store(&mut self) -> Vec<RemovalError>           once a store and a program are both present
│  │  └─ DiskStore::sweep(&self, Arc<CompiledGraph>) -> Vec<RemovalError>      [I/O]
│  ├─ ExecEngine::evict_cache(&mut self, ids) -> Vec<CacheNodeFailure>
│  │  └─ DiskStore::remove_nodes(&self, Vec<ProgramId>) -> Vec<RemovalError>   [I/O]
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
│  │  └─ BlobTarget::probe(&self) -> Option<BlobHeader>                        [I/O] from `Unknown`, and `Holds` under the stamp
│  └─ RuntimeCache::twin_leader(&self, NodeIdx) -> Option<NodeIdx>
├─ Executor::run(&mut self, RunRequest, &mut ExecOutcome)
│  └─ per node of `process_order`
│     ├─ task::yield_now()                                              before `Run | Reuse | Twin` only
│     ├─ ExecFrame::run_node(&mut self, NodeIdx)
│     │  ├─ ExecFrame::serve_reuse → RuntimeCache::hydrate_reuse -> ReuseOutcome
│     │  │  └─ BlobTarget::read(&self, Arc<Codecs>, BlobHeader, demand) -> Option<OutputSnapshot>  [I/O]
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
├─ Graph::detach_nodes(&mut self, &[NodeId]) -> Vec<DetachedNode>
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
- `graph (mod) ⇄ compound` — `add_func_node` and `add_instance` become one
  `Graph::add_seeded(node, &Ports)`.
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
`SharedAnyState`, both in the table; `BlobTarget::read` takes the `BlobHeader` that
`RuntimeCache.probed` keeps and `probe` returns; `GraphLoad` borrows one graph mutably and a
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
  digest changed, which then runs anyway, and keep a header for the load, which batch 5 keeps per
  run instead. A failed write, the one state that must outlive the value, is the slot's
  `BlobState::Failed`.
- **A blob operation is one blocking closure over `std` I/O, and a codec is synchronous.** A
  `tokio::fs::File` read is a blocking-pool hop of at most 2 MiB and a copy, and
  `BufWriter::seek` flushes: about 138 round trips and one extra copy of every byte for a 288 MiB
  image, each way, by the count of calls and not by a measurement. `common::task::run_blocking`,
  `common::fs::open` and `common::fs::publish` are the routes. Refused: buffering the `tokio`
  file, which keeps the hop per refill; a header after the body, which changes the byte layout to
  save the one seek left.
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
- **`InstancePath` and `ProgramId` share an in-crate `InlineIds`.** Refused: `arrayvec`, a new
  dependency for this crate and a `u32` length, 132 bytes for `ProgramId`'s 129.
- **A node with no name of its own shows its declaration's, whatever its kind.** A func renamed
  in the library then reads renamed on each node nobody named, as a renamed compound already does
  on its instances, the rule darkroom's `title` gives Blender's group node as its source. Refused:
  `Option<String>` with a func node still given its func's name at creation, which removes the
  sentinel and keeps two policies; the empty string as "the compound's".
- **A blob states a blake3 hash of each payload, checked at every load.** A blob is published by a
  rename with no sync, and `rename` promises only that the name "will be atomically replaced"
  ([rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html)), not that the data reached
  disk; ext4 forces it only for a replace, and only under `auto_da_alloc`
  ([ext4](https://www.kernel.org/doc/html/latest/admin-guide/ext4.html)). A pixel payload has no
  redundancy, so one with unwritten blocks decodes as a valid image. blake3 is in the tree and
  "much faster than MD5, SHA-1, SHA-2, SHA-3, and BLAKE2"
  ([BLAKE3](https://github.com/BLAKE3-team/BLAKE3)). Cost: one hash pass over each payload, each
  way, on the blocking pool, not measured here. Refused: a sync of the file before the rename, an
  `fsync` a stored blob that still misses a later corruption; no check.
- **A blob keeps its node's name, and an attach sweeps the rest.** The live set was already one
  blob per node; the sweep bounds the dead set with one `read_dir`. Cost: a node deleted and
  restored after the next attach recomputes, and twins store their payload twice. Refused: naming
  by digest under a size cap, as Bazel's disk cache, a content-addressable store that "can be
  garbage collected" ([remote caching](https://bazel.build/remote/caching)), and sccache, whose
  "default cache size is 10 gigabytes"
  ([local cache](https://github.com/mozilla/sccache/blob/main/docs/Local.md)): a cap setting,
  access times and a collector, a cache policy of its own; leaving it unbounded.
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

# Part 2 — Plan

Each batch leaves the build and the tests green. Whoever addresses an item deletes it.

## Batch 5 — blob and stamp I/O

Files: `data/codec/`, `exec/cache/disk_store/`, `exec/cache/stamp_job/`,
`exec/cache/runtime_cache/mod.rs`, `internals/blob.rs`, `library/tests.rs`,
`lens/src/image/codec/mod.rs`, `common/src/fs/mod.rs`, `clippy.toml`, `Cargo.toml` (`async-trait`
leaves `scenarium`).

- [ ] `disk_store/blob_target/mod.rs:98-106,193-229`, `format/mod.rs:76-139,266-296,353-382` —
  3 + K round trips to probe, about 8K + 7 to store, each payload byte copied once more. Target:
  the synchronous codec and format, one `run_blocking` per operation.
- [ ] `format/mod.rs:19-26,46-49` — a read checks magic, version, digest, id and lengths, and no
  payload byte: a blob with unwritten blocks is served as a hit. Target: `payload_hash`,
  `PayloadWriter`, `PayloadReader`, `FrameError::PayloadHash`. Tests: one flipped payload byte,
  and a zeroed tail, are each a miss that removes the blob; a codec that reads less than its
  payload still has the whole payload hashed.
- [ ] `format/mod.rs:99` — a `debug_assert_eq!` that flushes and seeks. Target: gone with the seek.
- [ ] `runtime_cache/mod.rs` `probe_reuse`, `hydrate_reuse` — the probe's header is read again
  at the load. Target: `RuntimeCache.probed`, passed to `BlobTarget::read`.
- [ ] `runtime_cache/mod.rs:178-180` — one `remove_file` hop per cone node. Target:
  `DiskStore::remove_nodes`.
- [ ] `disk_store/mod.rs:25-78` — no bound on dead blobs. Target: `DiskStore::sweep`,
  `ExecEngine.sweep_owed`. Tests: a blob of a removed node and a temp file over `STALE_TEMP` old
  are gone after an attach, a fresh temp file and each live node's blob stay; a store attached
  with no program removes nothing until the install.
- [ ] `stamp_job/mod.rs:152-205` — two passes, two `PathBuf`s and a path-walk stat per file.
  Target: one pass, `names` and `WalkedFile`, `common::fs::entry_metadata`.
- [ ] `format/tests.rs:41-101` — a 60-line chunked async double. Target: gone; a `Cursor`.

## Batch 6 — the wiring type and what repeats around it

Files: `graph/wiring.rs` (new), `graph/mod.rs`, `graph/detached.rs`, `graph/ghost_wiring.rs`,
`graph/port_map/`, `graph/interface_change.rs`, `graph/reconciled.rs`, `graph/inline_ids.rs` (new),
`graph/instance_path.rs`, `graph/program_id.rs`, `graph/node_decl.rs`, `graph/interface.rs`,
`graph/error.rs`, the `darkroom` callers, `darkroom/sample/`. After batch 4. The document's shape
changes; the sample is saved again.

- [ ] `graph/mod.rs:189-207`, `detached.rs:28-78`, `port_map/mod.rs:332-336` — the live tables as
  loose fields and as `Vec`s with a hand sort. Target: `Wiring`; `bindings` private.
- [ ] `darkroom/src/core/edit/bring_in.rs:168,203-204`, `core/io/export.rs:164` — writes to
  `graph.bindings` past `set_input_state`. Target: the setter.
- [ ] `graph/mod.rs:520-554`; `darkroom/src/core/edit/graph_intent/mod.rs:443-460`,
  `gui/pane/graph_ui/gesture/wire_ui/mod.rs:179-193`,
  `gui/pane/graph_ui/node_ui/port_row/mod.rs:815-832` — "both nodes live, admitted, no loop" by
  hand three times. Target: `Graph::admit_bind`; `admit_ghost` sugar over it.
- [ ] `graph/mod.rs:778-889,1287-1338`, `port_map/mod.rs:562-624` — a detach is four passes of
  every table per node: a redirect on 2000 nodes of 6000 bindings is about 50 M checks at load.
  Target: `detach_nodes`.
- [ ] `darkroom/src/gui/graph_ctx/node_ctx.rs:233-270,352-367` — the ghost tables scanned per node
  per frame, zero on a healthy document and up to about a millisecond at 400 ghosts. Target:
  `GhostWiring::wires_from` and `subscriptions_to`, cached per content revision by the caller.
- [ ] `interface_change.rs:45-92`, `graph/mod.rs:258-287`, `reconciled.rs:19-37` — four parallel
  vectors and three carrier types; two shapes for "keys a scope answered about"; a report built as
  a map, converted, and flattened back by `darkroom/src/core/document/mod.rs:606-623`. Target:
  `InterfaceChange.graphs`, `WiringChange`, `GraphSetReconciled.repairs`.
- [ ] `instance_path.rs:25-177`, `program_id.rs:40-183` — 110 lines twice. Target: `InlineIds`.
- [ ] `graph/mod.rs:581,793,812,1595-1612` — `input_bindings` beside `bindings_on`,
  `subscriptions_of` beside `subscriptions_from`, `is_subscribed` beside `subscription_state`,
  `reads_*` beside `shown_*`. Target: `Wiring`'s names; `shown_event` added; the rest deleted.
- [ ] `graph/mod.rs:1521-1590` — `ghost_notes` refreshes a ghost subscription's event name and not
  its trigger's. Target: both.
- [ ] `graph_set/mod.rs:487-497,103-117` — `validate_with` re-implements
  `ProgramShape::rejection`. Target: `GraphValidationError::Placement(PlacementRejection)`.
- [ ] `node_decl.rs:204-222`, `port_map/mod.rs:285`, `interface.rs:201-209` — a hand-rolled
  exact-size chain, a `&mut dyn Iterator` to share three collects, a serialize that clones the
  ports. Target: a mapped range, three collects, a borrowed repr.
- [ ] `graph/node/mod.rs:147-156,296-305` — an empty `name` meaning "the compound's", read in
  another crate, and a func node that copies its func's name at creation. Target:
  `Option<String>`, `Scope::title`, `GhostNode::title`. Test: a func renamed in the library and a
  renamed compound each read renamed on an unnamed node, and a named node keeps its name.
- [ ] `graph/mod.rs:761-762` — `produces_cycle` allocates a `Vec` and a set per hovered port.
  Target: a scratch the caller passes to `reaches`.
- [ ] `graph_set/mod.rs:507-516` — the manifest builds a `SavedDecl` per node and keeps one per
  func. Target: per func.
- [ ] `graph/tests.rs:254-268,780-791` — a removal checked by a count; `by_port` sorting away the
  documented repair order. Target: which wires survive; the order as written.
- [ ] Untested: `input_state_wire`, `shown_input` / `shown_output`, `count_touching`,
  `subscriptions_from`, `replace_compound`, `put_compound`, `rename_compound`, `admit_onto`, the
  first graph named when two pass a bound. Target: a case each.

## Batch 7 — declarations and the leaf types

Files: `library/`, `graph/func/`, `graph/ports.rs`, `graph/node_decl.rs`, `graph/value_domain/`,
`data/`, `containers/`, `exec/idx.rs`, `exec/compile/`, `exec/executor/mod.rs`, `builtin/`, the
`lens` and `darkroom` callers.

- [ ] `library/mod.rs:44-55,75-150` — one type to register into and to read. Target:
  `LibraryBuilder` and a `Library` with no mutators; fragment fns return or take a builder.
- [ ] `library/mod.rs:167-185`, `darkroom/src/gui/pane/interface_card.rs:66-70` — `data_types()`
  allocates and sorts with two hash lookups a comparison, per frame, and its caller collects again.
  Target: `&[DataType]`, built once.
- [ ] `library/mod.rs:99-117,285-339` — `redirect()` clones and composes the chain per call.
  Target: flattened at `build`.
- [ ] `graph/func/mod.rs:43-90`, `func_builder.rs:17-29,106-157` — ten fields written five times,
  each `pub` beside its setter, a `validate` with no fallible way to a `Func`. Target: `FuncDecl`,
  `checked` and `build`.
- [ ] `graph/func/mod.rs:30-36`, `ports.rs:212-216`, `node_decl.rs:152-222` — an event in two
  shapes and 70 lines to bridge them. Target: `Numbered<EventDecl>` and `event_lambdas`;
  `EventDecl::description`.
- [ ] `Func::into_builder`, `FuncBuilder::validate`, `Library::by_name`, `Library::type_count`,
  `Library::custom_type` — test-only on the published surface. Target: under `internals`.
- [ ] `data/dynamic_value.rs:115-193`, `executor/mod.rs:664-671` — a lambda that writes
  `Static(Int)` to an output declared `Custom(Image)` panics in its consumer, blamed on the
  consumer. Target: `output_types`, `DataType::holds`, `RunError::OutputType` naming the producer.
- [ ] `graph/func/value_variant.rs:7-37` — `name` is read only as `display`'s fallback and the one
  production constructor always sets `display`. Target: `{ label, value }`.
- [ ] `data/const_value.rs:152-165` — B13. Target: no `Display`; `DynamicValue`'s goes through
  `ValueText`.
- [ ] `graph/value_domain/float_span.rs:90-116` — B12. Target: `contains` requires a finite value;
  `fraction` computes in halves. Tests: ±∞, the full-range span.
- [ ] `graph/ports.rs:81`, `value_domain/tests.rs:161-165` — `#[serde(default)]` on a field every
  save writes, and a test decoding a file without it: a legacy deserializer. Target: both gone.
- [ ] `value_domain/int_span.rs:74-89`, `float_span.rs:119-135` — two hand-written `Deserialize`
  impls. Target: `#[serde(try_from)]`.
- [ ] `data/data_type/mod.rs:84-95`, `darkroom/src/gui/graph_ctx/input_ctx.rs:141-169`,
  `gui/pane/interface_card.rs:393` — "the constant a port starts with" answered twice, `Any` as
  `Null` in one and `""` in the other. Target: `Library::default_const`.
- [ ] `data/data_type/enum_type.rs:20-25` — `EnumType::new` checks nothing where `FileKind::new`
  asserts each entry non-empty and unique. Target: the same asserts.
- [ ] `dynamic_value.rs:221-225`, `lens/src/astro/nodes/stacking.rs:63`,
  `lens/src/config_node.rs:207` — a lambda that needs a string or a path list owned clones what
  the executor already cloned for it: `DynamicValue` has borrowed reads for eight kinds and one
  owning read. Target: `into_string`, `into_fs_paths`. The clone per invoke stays: the program is
  immutable and a lambda may take its inputs.
- [ ] `containers/col/mod.rs:31-64`, `consumer_cone/mod.rs:82`, `compiled_graph.rs:258`;
  `executor/mod.rs:496,649`, `validate.rs:50,53,56,99` — an index built three ways; `span.len as
  usize` at six sites. Target: the private field, `Span::len`.
- [ ] `exec/compile/validate.rs:24` — `program_ids` zipped with `e_nodes`, so a length mismatch
  truncates, and `search_sorted`'s order is never checked. Target: both asserted.
- [ ] `containers/col/tests.rs:2`, `idx_set/tests.rs:2` — the leaf's tests import `exec::idx`.
  Target: a local `idx_type!`.
- [ ] `exec/compile/mod.rs:381,336-338` — `totals.subscribers` counts authored subscriptions, not
  the flattened ones, and is the one total with no assert. Target: the flattened count, asserted.
- [ ] `exec/compile/mod.rs:421,536,437,151-157` — a sort that copies two 129-byte keys a
  comparison; a lowered constant cloned and thrown away; a doc that says a compile allocates
  "exactly the artifact". Target: `sort_unstable_by`; no clone; the doc lists what it allocates.
- [ ] `compiled_graph.rs:150-155` — `program_ids_within` filters every id for one contiguous run.
  Target: two `partition_point`s.
- [ ] `planner.rs:112-116`, `consumer_cone/mod.rs:70-92`, `schedule/mod.rs:461-462`,
  `executor/mod.rs:733-737`, `validate.rs:89-90` — "each wire into a node" written out six times.
  Target: `CompiledGraph::wires(node_idx) -> impl Iterator<Item = OutputAddr>`.
- [ ] `builtin/worker_events_library/mod.rs:17,90-97` — `FuncId::from_u128` and a hand-written
  boxed lambda beside `FuncId::literal` and `async_lambda!` everywhere else. Target: the two forms
  the rest use.
- [ ] `containers/unique.rs:6-8`, `compile/validate.rs:91`, `internals/program/mod.rs:87-88`,
  `internals/engine.rs:143,264-265` — comments naming what is gone ("a single id",
  `Compiler::resolve`, "a dependency cycle"). Target: trimmed.
- [ ] `library/tests.rs:295-363`, `func/tests.rs:221-259`, `system_library.rs:125-135` — tests of
  `FileKind`, `FuncInvoker` and `AnyState` in the library's file; tests that a setter stored its
  argument. Target: moved beside their type; the setter tests deleted.
- [ ] Untested: `LibraryBuilder::merge`, `data_types` order, `Col::search_sorted` and `reset`,
  `unique::extend`, an `IdxSet` of length 0, `Divide` by zero. Target: a case each.

## Batch 8 — the layers

Files: `identity/` (new), `decl/` (new), `graph/`, `library/`, `data/`, `runtime/`, `lib.rs`. After
batches 6 and 7.
Moves only; `lib.rs` keeps every published name.

- [ ] `library/mod.rs:30-34`, `func_redirect.rs:18` ⇄ `graph/{graph_set/mod.rs:54,
  port_map/mod.rs:35-36, interface_change.rs:39, type_names.rs:26, scope.rs:8}` — `graph ⇄
  library`. Target: `decl`.
- [ ] `graph/func/{lambda,event}.rs` ⇄ `runtime/ctx_manager.rs:3`, `runtime/log_entry.rs:3` —
  `graph ⇄ runtime`. Target: `ProgramId` in `identity`, `FuncRef` beside `FuncSignature`.
- [ ] `graph/func/mod.rs:13` ⇄ `graph/node/mod.rs:15`; `ports.rs:31-33`;
  `value_domain/mod.rs:22`; `identity.rs:21-24` — the cycles inside `graph`. Target: the moves
  under *Module layout*.
- [ ] `node/mod.rs:18-19` ⇄ `scope.rs:6`; `scope.rs:5` ⇄ `graph_set`; `graph/mod.rs:111` ⇄
  `compound.rs:34`; `error.rs:9` ⇄ `graph_set/mod.rs:36`. Target: `Scope::decl`, the compound
  table in `Scope`, `Graph::add_seeded`, `graph_set/program_shape.rs`.
- [ ] `data/data_type/` — `custom_type → codec → dynamic_value → custom_type`, `data_type ⇄
  enum_type`. Target: `EnumVariants` in `enum_type.rs`, `TypeRef` in `type_ref.rs`.
- [ ] `graph/func/{lambda,event,signature}.rs`, `ports.rs` — files not named for their type;
  `PortsError` outside an `error.rs`; test mods of 75 of 169 lines (`signature.rs`) and 36 of 87
  (`path_form.rs`). Target: `func_lambda.rs`, `event_lambda.rs`, `func_signature/{mod,tests}.rs`,
  `ports/error.rs`, `path_form/{mod,tests}.rs`.
- [ ] `graph_set/mod.rs` (950 lines) — `ProgramShape` with `shape_of` and `over_bounds`;
  `UnknownType` with `InterfacePort` and `unknown_types`. Target: `program_shape.rs`,
  `unknown_type.rs`.

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
