# Subgraphs (Composite Nodes) — Design

> A node that is itself a graph: a reusable group with a handful of exposed
> inputs/outputs that behaves like a single node in the parent, but expands to
> flat nodes before scheduling so dead-branch pruning, caching, and change
> propagation work *across* the boundary.

## 1. Problem statement

We want a `Node` that internally contains a `Graph`. From the parent's
perspective it has a fixed set of input ports and output ports and wires up like
any other node. Internally it is an arbitrary subgraph. Requirements:

1. **Encapsulation** — the subgraph exposes only chosen inputs/outputs; the rest
   is hidden. Reusable (place the same composite many times).
2. **Flat execution** — at execution time there must be *no* boundary. The
   subgraph's nodes participate in the same topological sort, the same
   `OutputUsage` Skip/Needed counting, the same cycle detection, and (future)
   the same change-pruning as the parent's nodes. A dead branch *inside* a
   composite whose output isn't consumed must be pruned exactly like a dead
   branch at top level.
3. **Recursion** — composites can contain composites, to arbitrary depth.

The key insight that drives the whole design: **Scenarium already has a
two-layer architecture** that maps perfectly onto this problem.

- `Graph` / `Node` / `Binding` — the *authoring* layer. UUID-keyed, stable,
  serialized, edited by the user (`graph.rs`).
- `ExecutionGraph` / `ExecutionNode` / `ExecutionBinding` — the *execution*
  layer. Index-keyed (`target_idx`), rebuilt from the authoring layer in
  `update()`, already flat, already does all the scheduling
  (`execution_graph/mod.rs`).

So the design is: **subgraphs are a first-class concept in the authoring layer,
and `ExecutionGraph::build_execution_nodes` *inlines* them into one flat
index space.** All the existing scheduling machinery — `walk_backward_*`,
`propagate_input_state_forward`, `OutputUsage`, cycle detection — keeps working
unchanged because by the time it runs, there are no subgraphs left, only a flat
sea of `ExecutionNode`s with rewritten bindings.

This is exactly the industry-standard approach (see §7).

## 2. How the current model works (grounding)

`graph.rs`:

```rust
struct Node    { id: NodeId, func_id: FuncId, inputs: Vec<Input>, events, ... }
struct Input   { name: String, binding: Binding }
enum   Binding { None, Const(StaticValue), Bind(PortAddress) }
struct PortAddress { target_id: NodeId, port_idx: usize }   // an upstream node's output
```

A `Node` is an *instance* of a `Func` (the definition, in `FuncLib`). Inputs
carry where their data comes from. Outputs are positional (`port_idx`), defined
by the `Func`.

`execution_graph/mod.rs` flattens this:

- `build_execution_nodes` walks `graph.iter()`, `compact_insert`s an
  `ExecutionNode` per `Node`, and rewrites each `Binding::Bind(PortAddress{target_id, port_idx})`
  into `ExecutionBinding::Bind(ExecutionPortAddress{ target_id, target_idx, port_idx })`
  where `target_idx` is the *array index* of the upstream `ExecutionNode`. The
  whole graph becomes index-addressable.
- Scheduling is three passes over that flat index space (collect order, forward
  state propagation, prune to consumer-needed order), then sequential async
  invocation.

Nothing in the scheduler knows about `Func` boundaries — it only sees
`ExecutionNode`s linked by `target_idx`. **That is why inlining is cheap: the
scheduler is already boundary-agnostic.**

## 3. Design options considered

### Option A — Nested execution (composite lambda runs a child ExecutionGraph)

Give the composite `Func` a real `FuncLambda` that owns a child
`ExecutionGraph`, feeds the parent's input values in, runs it, reads outputs
back. Minimal change to the scheduler.

**Rejected.** It reintroduces exactly the boundary requirement #2 forbids:

- Dead-branch pruning stops at the boundary. The parent decides whether to call
  the composite lambda as a unit; it can't prune a single dead output *inside*.
  `OutputUsage::Skip` for one of the composite's outputs can't propagate inward
  without a custom protocol threaded through the lambda signature.
- Change-pruning (the NOTES-AI "critical gap") would need a parallel
  implementation inside the nested engine.
- Caching granularity is the whole composite, not its interior nodes.
- Cross-boundary cycles (composite A's output feeds composite B feeds A through
  exposed ports) aren't detected by either engine alone.

It's simpler to *build* but it permanently forfeits the optimization the user
explicitly asked for. Only revisit if a composite must be a true black box
(e.g. a dynamically-sized loop body, see §8).

### Option B — Inlining / flattening at execution-graph build time (recommended)

Keep subgraphs purely in the authoring layer. When `ExecutionGraph::update`
builds the flat node list, **recursively expand each composite node into its
interior nodes**, rewriting bindings so boundary ports become direct
`target_idx` edges. The composite node itself produces *zero* `ExecutionNode`s —
it dissolves.

After expansion the `ExecutionGraph` is identical in kind to one authored flat
by hand. Every existing optimization applies for free, across all nesting
levels. This is the design developed below.

## 4. Authoring-layer data model

### 4.1 Boundary nodes (the Blender approach)

Borrow Blender's "Group Input / Group Output" pattern: a subgraph declares its
interface with two special built-in nodes.

```rust
// Well-known FuncIds for the two interface nodes (registered in FuncLib once).
const SUBGRAPH_INPUT_FUNC_ID:  FuncId = FuncId::from_u128(0x...);
const SUBGRAPH_OUTPUT_FUNC_ID: FuncId = FuncId::from_u128(0x...);
```

**Exactly one of each** (decided). A subgraph has at most one `SubgraphInput`
and one `SubgraphOutput` node; exposed-port *order* is just the port order on
that single node. Both are **optional**: a subgraph with 0 exposed inputs omits
`SubgraphInput` (a self-contained source), and one with 0 exposed outputs omits
`SubgraphOutput` (a sink — see terminal-ness in §4.6).

- **`SubgraphInput` node** — the inbound boundary. Carries *output* data ports
  (one per exposed input; interior nodes `Bind` to them to read "whatever the
  parent passed in") **and** *event inlets* (§4.5; interior nodes subscribe to
  them to react to parent-side events routed in).
- **`SubgraphOutput` node** — the outbound boundary. Carries *input* data ports
  (one per exposed output; interior nodes feed them, the parent reads them)
  **and** *event outlets* (§4.5; interior emitters surfaced as composite-level
  events the parent can subscribe to).

This is clean because it reuses the existing `Binding`/subscriber machinery
verbatim *inside* the subgraph. No new binding variant needed for interior
wiring. The only new thing is what those two nodes *mean* at inline time.

### 4.2 SubgraphDef — the reusable definition

A composite is a `Func`-like definition carrying a `Graph`:

```rust
id_type!(SubgraphId);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubgraphIo {
    pub name: String,
    pub data_type: DataType,
    pub required: bool,                  // inputs only
    pub default_value: Option<StaticValue>, // inputs only
}

/// One exposed event. An `inlet` routes a parent event inward (interior
/// nodes subscribe); an `outlet` surfaces an interior emitter outward
/// (parent nodes subscribe). At most one `Inlet` per def; any number of
/// `Outlet`s. See §4.5.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventDir {
    Inlet,   // parent event routed inward (interior nodes subscribe)
    Outlet,  // interior emitter surfaced (parent nodes subscribe)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubgraphEventIo {
    pub name: String,
    pub direction: EventDir,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubgraphDef {
    pub id: SubgraphId,
    pub name: String,
    pub category: String,

    /// The interior. Contains at most one SubgraphInput and one
    /// SubgraphOutput node (each omitted when its side exposes nothing).
    pub graph: Graph,

    /// Interface, in port order. `inputs[i]` <-> SubgraphInput output port i.
    /// `outputs[j]` <-> SubgraphOutput input port j. The *cached* interface;
    /// must stay in sync with the boundary nodes' arity (`validate_with`).
    pub inputs:  Vec<SubgraphIo>,
    pub outputs: Vec<SubgraphIo>,

    /// Exposed events (§4.5). Order matches the boundary nodes' event slots.
    pub events:  Vec<SubgraphEventIo>,
}
```

Behavior and terminal-ness are **not** stored here — they're derived from the
interior at inline time (§4.6).

Where do these live? **Both places — the storage location is what distinguishes
a *linked* (shared) instance from a *local* (detached) one (see §4.4).**

- **Library defs live in `FuncLib`.** Add `subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>`
  alongside `funcs`. This is the shared, reusable, "single source of truth"
  copy — the editor's node palette already lives in `FuncLib`. Editing it
  propagates to every linked instance (§4.4).
- **Local defs live in the `Document`.** A detached instance owns a private
  `SubgraphDef` stored in a Document-side `subgraphs` table (serialized and
  undoable, unlike `FuncLib` which is runtime-owned and shared across
  documents). Editing it affects only that instance.

The parent graph **never embeds the interior nodes directly** — an instance is
always a reference to a def (library or local). Materializing the interior into
the flat node list happens at execution-build time (§5), not in the authoring
graph. This is the single most important consequence of the design: it's what
makes "edit once → all instances update" automatic.

### 4.3 How a Node references a subgraph

`Node.func_id: FuncId` today is the sole "what am I" discriminant. Two ways to
let a node be a composite instance:

**Recommended: a `NodeKind` enum** so the authoring model is explicit and
serialization stays clean:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Func(FuncId),
    Subgraph(SubgraphRef),
}

/// Where an instance's definition is resolved from. The variant *is* the
/// link/local distinction — see §4.4.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubgraphRef {
    /// Shared library def in `FuncLib`. Editing the def updates every
    /// linked instance on the next `ExecutionGraph::update()`.
    Linked(SubgraphId),
    /// Private def in the owning `Document`. Editing it affects only this
    /// instance.
    Local(SubgraphId),
}

struct Node {
    id: NodeId,
    kind: NodeKind,           // was: func_id: FuncId
    name: String,
    behavior: NodeBehavior,
    inputs: Vec<Input>,       // arity = subgraph.inputs.len() for composites
    events: Vec<Event>,
    ...
}
```

A composite instance's `inputs` mirror `SubgraphDef.inputs` (same arity/order),
exactly like a func node's inputs mirror `Func.inputs` today. Its outputs are
positional per `SubgraphDef.outputs`. So **from the parent graph's view a
composite node is indistinguishable from a func node** — same `OutputPort`
wiring, same `From<&Func> for Node`-style constructor (`From<&SubgraphDef>`).

> Migration note: `func_id` is referenced widely (`graph.rs`, `execution_graph`,
> `validate_with`, darkroom UI, serialization). A staged migration: first add
> `kind` keeping a `fn func_id(&self) -> Option<FuncId>` shim, port call sites,
> then remove the shim. `#[serde(...)]` on the enum keeps old graphs loadable if
> you tag the `Func` variant as the default/untagged case.

### 4.4 Instancing & edit propagation (linked vs. local)

Two distinct things both get loosely called "materialize" — keep them apart:

1. **Execution-time materialization** *(always happens, §5).* Every instance is
   inlined into its own flat `ExecutionNode`s with per-instance ids
   (`flatten_id`). This is independent of everything below — it's how the
   subnodes "come to life" regardless of how the instance is stored.
2. **Authoring-time materialization** *(never, by default).* The parent `Graph`
   does **not** copy interior nodes; it holds a reference (`SubgraphRef`). Only
   "Make Local" (below) ever copies a def, and even then into a separate def
   table — never inlined into the parent graph.

Because the authoring graph holds only a reference and the inliner reads the def
fresh each `update()`, edit propagation is automatic and per-instance is just a
matter of *which def the reference points at*:

| Mode | `SubgraphRef` | Def stored in | Editing the def... |
|------|---------------|---------------|--------------------|
| **Linked** (default) | `Linked(id)` | `FuncLib.subgraphs` (shared) | updates **all** linked instances |
| **Local** (detached) | `Local(id)` | `Document.subgraphs` (private) | updates **only this** instance |

- **Make Local** (a.k.a. "make single user"): clone the library `SubgraphDef`
  into the Document's table under a fresh `SubgraphId`, flip the node to
  `Local(new_id)`. The instance is now editable in isolation.
- **Re-link**: point the node back at a `Linked(id)` and drop the local copy.

This is exactly the Blender (node-group user count + "Make Single User"),
Houdini (locked HDA vs. allow-editing-of-contents), and Unreal (material
function instance) model. There is no global linked-vs-local switch — it's a
per-node property with explicit conversions.

> **"Immutable in `FuncLib`" — what it should mean.** *Single source of truth:
> instances are references, not copies* — **not** "the def can never change."
> Editing the canonical library def is precisely how linked propagation works.
> The def is read-only only *during an execution run* (already guaranteed —
> `update()` borrows it `&`). A true content-immutable model (edit = mint a new
> version) would defeat shared editing, so it's not recommended; if a "snapshot
> this instance" feature is ever wanted, that's just Make Local.

**Shared structure ≠ shared state.** Two linked instances `C1`, `C2` of the same
def produce *different* `flatten_id`s — their `instance_path`s are `[C1]` vs.
`[C2]` — so at runtime they hold completely independent `output_values` caches
and `AnyState`, even though they were stamped from one def. Correct: they're fed
different inputs and must compute independently. `CompactInsert`'s id-matching
means an edit to a linked def invalidates only the changed interior subtree, and
it does so consistently across every instance on the next `update()`.

**Editor burden (darkroom, not the engine).** Drilling into a `Linked` composite
edits the `FuncLib` def — which affects *all* instances and *other open
documents* sharing that library. The editor must surface the user count and
offer Make Local before edits, and route edits to `FuncLib.subgraphs` (Linked)
vs. `Document.subgraphs` (Local) accordingly. The engine side stays uniform: the
inliner just resolves a `SubgraphRef` to a `&SubgraphDef` from the right table.

### 4.5 Events across the boundary

Events are **internal by default** — an interior node's events fire and trigger
interior subscribers, invisibly to the parent. But, like data ports, an event
can be *exposed* on the composite interface, in either direction:

- **Outlet** (surface upward, any number). An interior emitter's event becomes a
  composite-level event the parent can subscribe to. The composite Node carries
  this in its `events: Vec<Event>` exactly like a func node — the parent's
  subscribers live there. The outlet maps `composite.event[j]` →
  interior `EventRef { node_id, event_idx }`, recorded as an event slot on the
  `SubgraphOutput` node.
- **Inlet** (route inward, **at most one** — decided). A single shared event the
  composite "receives" and routes to interior subscribers. Interior nodes
  subscribe to *one* event slot on the `SubgraphInput` node ("when this composite
  is triggered, run me"); from the parent side the composite is added as a plain
  subscriber to whatever parent emitter feeds it.

  **One inlet keeps the core `Event` model unchanged.** Today a subscriber is a
  bare `NodeId` (`Event.subscribers: Vec<NodeId>`) with no "which of my slots"
  index, so a composite-as-subscriber is unambiguous only with a single inlet.
  Supporting multiple inlets would force `subscribers` to carry an inlet index —
  explicitly out of scope. (`SubgraphDef.events` therefore holds 0..N `Outlet`
  entries and 0..1 `Inlet`.)

Both reduce to the **same subscriber-edge short-circuit the inliner already does
for data edges** (§5.2): an event slot on a boundary node emits no
`ExecutionNode` and holds no real subscribers itself — at inline time its
subscriber set is spliced onto the real emitter on the other side, with the same
`flatten_id` id-remap that data `Bind` edges get. So:

- An **outlet**: parent subscribers of `composite.event[j]` are rewritten to
  subscribe to `flatten_id(path, interior.node_id)` event `interior.event_idx`.
- The **inlet**: interior subscribers of the `SubgraphInput` event slot are
  spliced onto whatever parent emitter the composite subscribes to (resolved at
  the enclosing level, `path` minus its last element).

After inlining there are no composite events left — only direct emitter→subscriber
edges in the flat graph, which `active_event_triggers` and the worker's event
loop consume unchanged. Events that are *not* exposed never touch the boundary
and stay entirely within the instance.

### 4.6 Derived properties (terminal, behavior)

A composite's `terminal` flag and execution behavior are **computed from the
interior**, never declared on `SubgraphDef` (so they can't drift):

- **Terminal** — a composite is terminal iff **any interior node is terminal**
  (decided). A subgraph wrapping a `print`/sink node is itself a sink and gets
  collected as a terminal root in `collect_terminal_nodes`. With inlining this
  is automatic: the interior terminal node *is* a real `ExecutionNode`, so it's
  picked up directly — the composite wrapper never needed a flag at all. (The
  derived flag matters only for editor display and palette filtering.)
- **Behavior / purity** — `Pure` only if **every** interior node is `Pure`;
  otherwise `Impure` (decided default). Again automatic under inlining: each
  interior node keeps its own behavior and caches independently, so there's no
  single composite-level cache decision to make. The derived value is for
  display/validation only.

## 5. Inlining algorithm (the core)

All the new complexity lives in one place: `ExecutionGraph::build_execution_nodes`.
Today it does one pass over `graph.iter()`. We replace it with a recursive
flattening pass.

### 5.1 Identity remapping — the central trick

When we inline a composite instance, its interior `NodeId`s would collide if the
same `SubgraphDef` is placed twice. So interior nodes get **synthesized,
deterministic execution identities** by combining the instance path with the
interior id:

```rust
// A flattened node's identity = the chain of composite-instance NodeIds you
// descended through, plus the leaf interior NodeId.
fn flatten_id(instance_path: &[NodeId], interior: NodeId) -> NodeId {
    // Deterministic hash so re-runs/serialization are stable and CompactInsert
    // can match nodes across updates (preserves cached output_values!).
    let mut h = Fnv::new();
    for id in instance_path { h.update(id.as_u128()); }
    h.update(interior.as_u128());
    NodeId::from_u128(h.finish_u128())
}
```

Determinism matters: `CompactInsert` (the existing incremental-update path)
matches `ExecutionNode`s by `NodeId` across `update()` calls to preserve
`output_values` caches. A stable synthesized id means editing one composite
instance doesn't blow away the cache of its sibling instances.

Top-level nodes keep their own id (`instance_path = []` → just the interior id,
which for a real top-level node is itself — define `flatten_id(&[], n) == n`).

### 5.2 Binding rewrite rules

Walk the parent graph recursively. Maintain `instance_path: Vec<NodeId>`. For
each node encountered at the current level:

1. **Plain func node** → emit one `ExecutionNode`. Its id is
   `flatten_id(path, node.id)`. Rewrite each input binding:
   - `Const` / `None` → unchanged.
   - `Bind(OutputPort{node_id, port_idx})` → resolve `node_id` *at this
     level* (see resolution table below), producing the flattened target id and
     possibly a different `port_idx`.

2. **Composite node** → emit *nothing for the node itself*. Resolve its
   `SubgraphRef` to a `&SubgraphDef` (`Linked` → `FuncLib.subgraphs`, `Local` →
   `Document.subgraphs`), then recurse into `def.graph` with
   `path' = path ++ [node.id]`. The boundary nodes are handled specially (they
   also emit nothing — they're pure wiring). Whether the def is linked or local
   makes no difference to the inliner past this resolution step — both are just a
   `&SubgraphDef`, so two linked instances and one local instance flatten
   identically, each under its own `instance_path`.

The binding resolver is the heart of it. When some interior/sibling node binds
to `target_id @ port_idx`, classify `target_id`:

| `target_id` is...                        | Rewrite the edge to point at...                                                                 |
|------------------------------------------|-------------------------------------------------------------------------------------------------|
| a plain func node at this level          | `flatten_id(path, target_id)`, same `port_idx`                                                  |
| a composite node at this level           | **follow into it**: the composite's output `port_idx` maps to its `SubgraphOutput` input `port_idx`; resolve *that* input's binding inside the child (path ++ [target_id]) and use whatever it points to |
| the `SubgraphInput` node (interior only) | **follow out**: output `port_idx` of SubgraphInput = exposed input `port_idx` of the *enclosing instance*; resolve that instance's input binding at the parent level (path without last) |

In other words: **boundary nodes are short-circuited.** A `SubgraphInput`
output is replaced by whatever the parent fed into that port; a composite's
output is replaced by whatever its `SubgraphOutput` input was fed from. The
edges "jump over" the boundary nodes so no `ExecutionNode` is created for them.
After the pass, the flat graph has direct producer→consumer edges spanning
arbitrary nesting depth.

### 5.3 Worked example

```
Parent graph:
  A (func)  ──┐
              ├──►  C := Composite{ in0, in1 → out0 }  ──►  D (func, terminal)
  B (func)  ──┘

Composite C interior:
  SubgraphInput[out0=in0, out1=in1]
        out0 ─► X (func) ─┐
        out1 ─────────────┤
                          ├─► Y (func) ─► SubgraphOutput[in0]
```

Inlined flat graph (boundary nodes gone, ids = flatten_id):

```
A ─► X'         (X's in bound to SubgraphInput.out0  → rewritten to A)
A? B ─► Y'      (Y's ins: one from X', one from SubgraphInput.out1 → rewritten to B)
Y' ─► D         (D's in bound to C.out0 → SubgraphOutput.in0 → rewritten to Y')
```

`X' = flatten_id([C], X)`, `Y' = flatten_id([C], Y)`. Now if `D` doesn't consume
`C.out0`, the existing `walk_backward_collect_order` never reaches `Y'` or `X'`
→ **dead interior branch pruned for free.** Exactly requirement #2.

### 5.4 Cycle detection

No new code. Cross-boundary cycles become ordinary cycles in the flat graph and
are caught by the existing `Color::Gray` DFS in `walk_backward_collect_order`.
One *authoring-time* guard to add: a composite cannot (transitively) contain
itself — detect recursion when expanding `instance_path` (if a `SubgraphId`
appears twice on the descent path, error). Catch this in `Graph::validate_with`
and at edit time so the editor can refuse the connection.

### 5.5 What stays untouched

`prepare_execution`, `propagate_input_state_forward`,
`walk_backward_collect_execute_order`, `run_execution`, `OutputUsage`,
change-pruning (when added), event subscriber wiring — **none change.** They
operate on the flat `e_nodes` array, which now just happens to contain inlined
interior nodes. This is the payoff of doing the work at build time.

Event subscribers (`Event.subscribers: Vec<NodeId>`) need the same id remapping
as bindings — a subscriber id crossing into/out of a composite must be flattened
with the right `instance_path`, and exposed-event inlets/outlets short-circuit
the boundary just like data edges. See §4.5; it's the same resolver.

## 6. Editor / darkroom implications

- **Drill-in navigation.** The editor shows a composite as one node; double-click
  descends into its `SubgraphDef.graph` (breadcrumb = `instance_path`). Standard
  in Houdini/Blender/Nuke.
- **Interface editing.** Adding/removing/reordering a `SubgraphInput` output
  port or `SubgraphOutput` input port updates `SubgraphDef.inputs/outputs` and
  therefore the arity of *every* linked instance. Treat like a func signature
  change and **reconcile bindings by port name** (decided): match each surviving
  instance input to the new interface by name (so reorders preserve wiring),
  drop bindings whose port was removed, default-initialize newly added ports.
  Name is the identity key — renaming a port reads as remove+add (binding lost),
  which is the conventional, predictable behavior.
- **Argument values / previews.** `get_argument_values(node_id)` works on
  flattened ids. To inspect an interior node the editor passes the flattened id
  (`flatten_id(path, interior)`); the breadcrumb path gives it everything needed
  to compute that. Inspecting a composite *instance's* exposed output = inspect
  the flattened node its `SubgraphOutput` resolves to.
- **Selection/stats** in `ExecutionStats` already key by `NodeId`; flattened ids
  flow through unchanged, the editor maps them back via the path.

## 7. Industry precedent (confirms the approach)

- **Blender node groups** — a NodeGroup resource holds the subgraph plus
  Group Input / Group Output interface nodes; instances reference the group. Our
  §4.1 boundary nodes + §4.2 `SubgraphDef` are a direct analog.
- **Houdini subnets** — subnetworks with input/output connectors; the cook
  (execution) sees a flattened dependency network. Houdini's *Compile Block /
  Invoke Compiled Graph* explicitly flattens a subnet for efficient repeated
  evaluation — same "flatten before scheduling" idea.
- **DaCe Stateful Dataflow multiGraphs** — "scope" nodes wrap an arbitrary
  subgraph by dominating/post-dominating it: every edge from outside passes
  through entry/exit nodes. That is precisely our SubgraphInput/SubgraphOutput
  short-circuit, and they likewise expand scopes for scheduling.
- **LLVM/MLIR inlining** — the general principle: inline call sites into one IR,
  then let the existing whole-program passes (DCE = our dead-branch pruning)
  operate without knowing a call boundary ever existed.

Common thread across all of them: **define with a boundary, schedule without
one.** That is Option B.

## 8. Future / out-of-scope

- **Iterating composites (for-each / loop subnets).** A composite whose interior
  runs N times over a collection *cannot* be statically inlined (N is a runtime
  value). That case wants Option A (nested execution) deliberately — the loop
  body is a genuine black box invoked per element. Worth keeping the
  nested-execution path in mind as a complementary mechanism, not a competitor.
  Houdini splits exactly this way (subnet = inlined; for-each block = compiled
  and invoked).
- **Cross-instance cache sharing.** Two instances of the same `SubgraphDef` with
  identical inputs currently produce distinct flattened ids → distinct caches.
  A content-addressed cache key (hash of func + input values) could dedupe, but
  that's the broader change-pruning work in NOTES-AI, not subgraph-specific.
- **Recursion depth / explosion.** Inlining a deeply reused composite multiplies
  node count. Fine for tens–hundreds of nodes; if graphs get huge, revisit with
  Option A for hot subtrees.

## 9. Recommendation & rollout

Adopt **Option B (build-time inlining)** with the **Blender-style boundary
nodes** and a `SubgraphDef` referenced via a new `NodeKind::Subgraph(SubgraphRef)`
discriminant — instances are **always references, never embedded copies** —
resolved from `FuncLib.subgraphs` (linked) or `Document.subgraphs` (local).

Suggested staging:

1. **Authoring model** — add `SubgraphId`, `SubgraphDef` (+ `SubgraphIo`,
   `SubgraphEventIo`/`EventDir`), the two boundary built-in funcs (one-of-each,
   both optional), `NodeKind` + `SubgraphRef` (with the `func_id` shim), the
   `FuncLib.subgraphs` and `Document.subgraphs` tables. Serialization +
   `validate_with` (interface↔boundary arity, ≤1 event inlet, recursion guard —
   a `SubgraphId` may not appear twice on a descent path). Tests: round-trip,
   recursion rejection, zero-input and zero-output subgraphs.
2. **Inliner** — rewrite `build_execution_nodes` into the recursive flattener
   with `flatten_id` + the binding resolver + `SubgraphRef` resolution + event
   inlet/outlet short-circuit. Derive terminal/behavior from the interior. Keep
   `CompactInsert` for cache preservation. Tests: the §5.3 example asserting
   exact flat node set + edges; dead interior branch pruned; cross-boundary cycle
   detected; nested (2+ deep); two linked instances don't share/clobber caches;
   editing a linked def re-inlines all instances while preserving unchanged
   subtrees' caches; interior terminal makes the composite a terminal root;
   exposed event outlet fires a parent subscriber.
3. **Editor** — drill-in navigation, interface editing with **by-name binding
   reconciliation**, argument-value inspection via flattened ids, **Make Local /
   user-count UI** and edit-routing to the right def table. (darkroom — separate
   from engine correctness.)

Everything downstream of the flat `e_nodes` array — the scheduler, caching,
change-pruning, events — is untouched, which is the whole reason this design is
worth it.
