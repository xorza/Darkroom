# Dataflow-as-code: serializing graphs as executable Rhai scripts

Status: **proposal / investigation**. No code yet. This documents the idea,
the prior art, how it maps onto scenarium's actual model, the hard problems,
and a recommended phased plan.

## TL;DR

- The idea — emit a graph as an *executable* script (`r = node1(a, b)`) and
  rebuild it by **tracing**: run the script with proxy stand-ins for node
  functions and values that *record* nodes/edges instead of computing — is
  sound and has heavy industrial precedent (PyTorch FX, JAX, Dask `delayed`,
  Bazel Starlark). The proxy "self-wrap" trick maps **1:1** onto scenarium's
  `Binding::Bind` edges.
- But pure tracing **loses node identity, layout, and every non-dataflow
  structure** (events/subscriptions), and creates a code-vs-graph
  source-of-truth tension. Every shipping node tool (Unreal, Blender,
  Houdini) resolves this by keeping the **graph canonical** and treating text
  as a faithful serialization *with ids/positions embedded* — not as
  re-traced source logic.
- Recommendation: pursue it as a **graph-canonical, losslessly
  round-trippable** script format. Carry identity/metadata explicitly (stable
  per-node key = the script variable name; positions in a sidecar/attribute),
  reuse tracing as the *import* path, and phase it in behind the existing
  data formats (which stay as the robust fallback). Tracing-with-control-flow
  becomes an *authoring convenience*, not the canonical on-disk shape.

## 1. Motivation

Today a `Graph` round-trips through `common`'s `SerdeFormat`
(`common/src/file_format.rs:19`) via plain `serde`. Rhai is one of the
formats but is used **as a data language only**: `serde_rhai`
(`common/src/serde_rhai/mod.rs`) evaluates the file with
`engine.eval_expression(...)` — a locked-down, data-only expression parse (no
variables, calls, or modules) routed through `serde_json::Value`. The output
is a verbose object-literal table keyed by UUIDs. A *single 1-input add inside
a subgraph* looks like this (`darkroom.library.rhai`):

```rhai
#{ subgraphs: [ #{ graph: #{
  bindings: [
    [ #{ node_id: "0d45de52-…", port_idx: 0 },
      #{ Bind: #{ node_id: "3ce1c261-…", port_idx: 0 } } ],
    [ #{ node_id: "3ce1c261-…", port_idx: 1 },
      #{ Const: #{ Float: 1.0 } } ],
  ],
  nodes: [
    #{ behavior: "AsFunction", id: "2e9b285d-…", kind: "SubgraphInput", name: "" },
    #{ behavior: "AsFunction", id: "3ce1c261-…", kind: #{ … }, name: "add" },
  ],
  subscriptions: [],
} } ] }
```

This is machine-perfect but human-hostile: UUIDs everywhere, edges expressed
as `(port, binding)` pairs detached from the nodes they connect, no visual
sense of data flow. The proposal is the *same graph* as:

```rhai
// subgraph "increment": one input -> add 1.0
fn increment(x) {
    add(x, 1.0)
}
```

where data flow is implicit in how values thread through calls.

## 2. The shape of the proposed format

A worked root-graph example. Source graph: constant inputs `a = 3`, `b = 5`;
`n1 = add(a, b)`; `n2 = mul(a, n1)`; `n2` feeds a terminal `print`.

```rhai
let a = 3.0;
let b = 5.0;
let n1 = add(a, b);          // add: inputs (lhs, rhs) -> out
let n2 = mul(a, n1);         // mul: reuse `a` (const) and `n1` (edge)
print(n2);                   // terminal sink
```

The semantics we want on **load** (tracing): `add`, `mul`, `print` are *not*
the real functions — they are registered proxies that, when called, append a
`Node` to a `Graph` under construction and return a `NodeRef` proxy standing
for that node's output port. Passing `n1` (a `NodeRef`) as an argument records
a `Binding::Bind`; passing `a` (a number) records a `Binding::Const`. Nothing
is computed.

### Mapping script constructs → scenarium model

| Script | scenarium (`scenarium/src/graph.rs`, `data.rs`, `function.rs`) |
|---|---|
| `add(...)` call | a `Node { kind: NodeKind::Func(FuncId) }`; the name resolves to a `FuncId` via `FuncLib` |
| call returns `NodeRef` | a proxy for `OutputPort { node_id, port_idx: 0 }` (the FX "node wrapped back into a proxy" trick) |
| `let n1 = …` | binds the script variable to that `NodeRef`; the **variable name is the node's stable key** (see §5) |
| numeric / string / bool literal arg | `Binding::Const(StaticValue::{Float,Int,Bool,String,…})` on that input port (`graph.rs:48`, `data.rs:118`) |
| `NodeRef` arg | `Binding::Bind(OutputPort)` on that input port |
| free identifier not `let`-bound | resolved by `on_var` as a graph **input** (subgraph boundary) — see §8 |
| `fn name(args){…}` | a `SubgraphDef` (`subgraph.rs:67`); params ↔ `SubgraphInput`, returned values ↔ `SubgraphOutput` |
| `subscribe(emitter.evt, sub)` statement | a `Subscription { emitter, event_idx, subscriber }` (`graph.rs:58`) — events are *not* dataflow (see §8) |
| `.once()`, `.disabled()` modifiers on a `NodeRef` | per-node `behavior: NodeBehavior::Once` / `disabled: bool` (`graph.rs:126`) |

The key realization: scenarium's wiring is already SSA-shaped. `bindings:
BTreeMap<InputPort, Binding>` (`graph.rs:182`) is exactly "each consumer input
is either a constant or a reference to one producer output" — which is what
`let r = f(x)` expresses. Tracing is a natural fit for the **data** half of the
model.

## 3. Prior art (what to copy, what to fear)

Full citations in §11. The short version:

- **PyTorch `torch.fx`** is the closest analog and the design to copy. Symbolic
  tracing wraps args in `Proxy` objects; operations on a proxy *record a Node*
  into a `Graph` and **the new node is itself wrapped in a Proxy** so it can be
  used downstream. That self-wrap is precisely how `n1 = add(a,b)` becomes
  usable in `mul(a, n1)`. FX also generates source *from* the graph — the
  inverse direction we need for codegen.
- **JAX** formalizes the proxy as a `Tracer` carrying an *abstract value*
  (shape/type, no data) and records a `jaxpr`. Lesson: separate **static**
  (decided at trace time, shapes the topology) from **traced** (data flowing
  between nodes).
- **Dask `delayed`** is the practitioner template: a decorator makes calls
  lazy, "delayed objects act as proxies… all operations are done lazily by
  building up a graph," realized later with `.compute()`. This *is* our model.
- **Bazel/Buck Starlark** is the template for making *execute-to-build* safe
  enough to be a file format: deterministic, hermetic (no clock/RNG/IO),
  frozen-after-run. Steal these constraints.
- **The round-trip trap.** `torch.jit.trace` and FX both **unroll loops and
  bake one branch** of any data-dependent `if` — "the trace only unrolls the
  loop for the number of iterations it ran." Promoted to a file format, this
  means **graph → script → graph cannot recover a hand-written loop**. The
  tools that preserve control flow (`torch.jit.script`, TF AutoGraph) do it by
  **parsing the AST**, not tracing.
- **Industry round-trip stance.** Unreal Blueprints, Blender Geometry Nodes,
  Houdini all keep the **graph canonical**; their "text" forms (e.g. Unreal's
  T3D clipboard) are graph dumps *with node positions and ids embedded*, not
  higher-level programs that get re-traced. No mainstream node tool
  auto-regenerates its source-of-truth code from the graph and back.

### In-repo precedent

We have already executed Rhai to manipulate graphs — in the **frozen** editor.
`darkroom-egui-deprecared/src/script/mod.rs` builds a sandboxed `rhai::Engine`
(`register_fn` for `run`/`shutdown`/`list_funcs`, a prelude loaded via
`Module::eval_ast_as_new` + `register_global_module`, `eval_with_scope`, and an
explicit deeply-nested-AST DoS guard). Its `prelude.rhai` exposes an
**imperative** graph API:

```rhai
fn create_node(func_id, x, y) { … apply(action); action.AddNode.view_node.id }
fn connect(out_node, out_port, in_node, in_port) {
    apply(#{ SetInput: #{ node_id: in_node, input_idx: in_port,
        to: #{ Bind: #{ target_id: out_node, port_idx: out_port } } } });
}
```

That is the *imperative cousin* of this proposal (mutations, not dataflow
returns) and proves the engine-embedding, sandboxing, and intent-mapping
machinery is well-trodden ground here. The new idea is to make the surface
**declarative dataflow** and to **trace** it rather than execute side effects.

## 4. The tracer, concretely (Rhai mechanics)

Rhai has every primitive we need except a function catch-all.

- **One `register_fn` per func type, generated from `FuncLib`.** There is *no*
  generic interception of unregistered names in Rhai — dispatch is a typed hash
  lookup. So at load time, iterate `FuncLib.funcs` and register a Rhai function
  per func whose body closes over the `FuncId` and pushes a node into the trace
  state. (Name collisions: `FuncLib::by_name` is a linear search and names are
  **not unique** — see §7.)
- **`NodeRef` is a `register_type` custom type** (only requirement: `Clone`).
  Returned from every node-fn; this is the proxy. Multi-output access via
  `register_get`/`register_indexer_get` so `n.sum` or `n["sum"]` yields an
  `OutputPort` proxy (§6).
- **`on_var` resolves graph inputs.** Free identifiers that aren't `let`-bound
  become input proxies:
  `Engine::on_var(|name, _idx, _ctx| -> Result<Option<Dynamic>, Box<EvalAltResult>>)`,
  returning `Ok(Some(input_ref))`. Pair with **Strict Variables Mode** so every
  undeclared name *must* be a declared input or it's a compile error. (Note:
  values from `on_var` are treated as constants — keep mutable trace state in a
  host-side `Scope`/`Arc`, not in the returned `Dynamic`.)
- **Object maps `#{ … }` are the named-argument & metadata vehicle** —
  `add(a, b, #{ id: "n1", pos: [120, 40] })`. Ideal for inline identity/layout
  (§5) and for optional/named inputs (§6).
- **Operator overloading is optional sugar.** With
  `Engine::set_fast_operators(false)`, `c = a + b` can dispatch to a traced
  `add`. Nice for math-heavy graphs; not required.
- **Sandbox like Starlark.** `set_max_operations`, `set_max_call_levels`,
  `set_max_expr_depths`, `disable_symbol("eval")`, and register **no**
  clock/RNG/IO. Rhai is sandboxed by default; the deprecated engine already
  sets analogous limits.

Sketch of the trace state and a generated node-fn:

```rust
// Host-side, shared across the whole evaluation.
struct Tracer { graph: Graph, func_lib: Arc<FuncLib>, names: HashMap<NodeId, String> }

#[derive(Clone)]                       // Rhai custom type
struct NodeRef { node: NodeId, port: usize }

// Registered once per Func, closing over its FuncId:
engine.register_fn(func_name, move |args: &[Dynamic]| -> NodeRef {
    let id = NodeId::unique();
    tracer.borrow_mut().graph.add(Node::func(id, func_id));
    for (i, arg) in args.iter().enumerate() {
        let binding = if let Some(nref) = arg.try_cast::<NodeRef>() {
            Binding::Bind(OutputPort { node_id: nref.node, port_idx: nref.port })
        } else {
            Binding::Const(static_value_from_dynamic(arg))   // literal -> Const
        };
        tracer.borrow_mut().graph.set_input_binding(InputPort { node_id: id, port_idx: i }, binding);
    }
    NodeRef { node: id, port: 0 }       // FX self-wrap: return a proxy for out[0]
});
```

## 5. Hard problem: node identity & stable keys

The current data format preserves `NodeId` UUIDs verbatim across save/load, and
darkroom keys **all view state** off `NodeId`: `ViewNode { id, pos }`,
`GraphView { view_nodes, pan, scale, selected_nodes }`,
`Document { graph, main_view, sub_views, tabs, active }`
(`darkroom/src/document/`). A name-based script does **not** naturally carry
UUIDs, and **trace order is a fragile identity** (reorder the script → every
node renames → positions/selection/undo break, diffs explode).

Recommendation: make the **script variable name the durable per-node key**.
- Codegen assigns each node a stable, readable variable (`n1`, `add_2`, or a
  user-given label) and writes it deterministically.
- On load, mint fresh `NodeId`s but record `NodeId → variable-name`; darkroom
  stores positions/selection keyed by **variable name**, not UUID.
- For exactness across FuncLib churn, optionally also emit the original
  `NodeId`/`FuncId` in a trailing `#{ id: …, func: … }` object-map arg and
  prefer it when present, falling back to name lookup. (Unreal-T3D approach:
  identity travels *with* the dump.)

This makes the file diff-friendly and lets a human rename a node by renaming a
variable, while keeping positions attached.

## 6. Hard problem: multiplicity (outputs, optional/named inputs)

- **Multiple outputs.** scenarium nodes have `Vec<FuncOutput>` (`function.rs`),
  addressed by `port_idx`. A bare `NodeRef` defaults to `out[0]`; expose the
  rest as named getters/indexers: `n.quotient` / `n.remainder` →
  `OutputPort{port_idx}`. (Rhai has no native tuple destructuring; prefer named
  getters over `let (q, r) = …`.)
- **Optional / unbound inputs.** `FuncInput { required, default_value, … }`
  means some ports may be absent (`Binding::None` = not in the map). Positional
  `f(a, b)` can't express "skip input 1." Use **object-map named args** —
  `f(#{ lhs: a, scale: 2.0 })` — keyed by `FuncInput.name`; omitted keys stay
  `Binding::None`. Recommend object-map as the canonical call form and treat
  positional as sugar for the all-required prefix.
- **Const sharing.** A top-level `let a = 3` reused by two nodes lowers to a
  `Binding::Const` on *each* consumer port (scenarium has no shared-constant
  node; const lives per input port). If a "value/constant" builtin exists
  (`basic_funclib` has `value`-input funcs), codegen could instead emit one
  value node and `Bind` both consumers — make this a documented, optional
  normalization, not silent behavior.

## 7. Hard problem: func-name stability vs UUID exactness

The script calls funcs **by name**, but `FuncId` is the stable identity and
`FuncLib::by_name` is a non-unique linear search. Names can collide (two
`add`s) or drift (a func renamed in the library). Options, in order of
robustness:
1. Emit `name` for readability **and** the `FuncId` in the call's metadata map;
   resolve by id, fall back to name. (Recommended — human-readable, exact.)
2. Namespace by category: `math::add`. Helps collisions, not renames.
3. Name-only. Most readable, least stable; acceptable only if the FuncLib
   guarantees unique stable names (it currently does not).

## 8. Hard problem: non-DAG structure (events, inputs, cycles)

- **Events/subscriptions are not dataflow.** `Subscription { emitter,
  event_idx, subscriber }` (`graph.rs:58`) is an async trigger edge, *not* a
  value passed through a return. It cannot be expressed as `r = f(x)`. Emit it
  as an explicit statement: `subscribe(emitter_var.on_done, subscriber_var)`,
  traced into a `Subscription`. `SubgraphEvent`s (`subgraph.rs:36`) that
  re-expose an interior emitter become part of the `fn`'s declared interface.
- **Graph inputs exist only for subgraphs.** The root `Graph` has no parameter
  interface; root-level `let a = 3` is just an inline constant. A
  **subgraph**'s inputs are first-class `SubgraphInput` boundary nodes
  (`graph.rs` boundary kinds) — these map cleanly to `fn` **parameters**, and
  `SubgraphOutput` to the `fn`'s **returned** `NodeRef`(s). `on_var` is only
  needed if we also allow free root-level inputs.
- **Cycles.** Data-binding cycles are already rejected by the planner
  (`execution/planner.rs`), so straight-line dataflow (inherently acyclic)
  loses nothing on the data side. Feedback that *does* exist goes through
  events, handled above.

## 9. Hard problem: control flow & the source-of-truth decision

This is the crux. If users may **hand-write control flow** in the script (a
`for` that stamps out 8 nodes), tracing will **unroll** it; `graph → script →
graph` then erases the loop — the FX/`jit.trace` failure mode at the
file-format level. Two coherent stances:

- **(A) Graph is canonical (recommended).** The `.rhai` script is a *faithful,
  regenerable serialization* of the graph — codegen emits straight-line
  dataflow with ids/positions embedded; the loader traces it back losslessly.
  No data-dependent control flow in the canonical file. This matches every
  shipping node tool and sidesteps unrolling/identity/nondeterminism. Tracing
  with loops/conditionals is offered only as an **import convenience** ("paste a
  dataflow snippet, we trace it into nodes once"), clearly one-way.
- **(B) Code is canonical.** Users own the script; the graph is a derived view.
  Then you should **parse the AST** (not trace) to preserve loops/comments and
  get source-span errors — a much larger effort, and it breaks the visual
  editor's ability to be the primary authoring surface.

Recommendation: **(A)**. It keeps the visual editor authoritative, gives a
clean human-readable/diffable file, and still delivers the ergonomic
`r = node(a, b)` surface.

## 10. Architecture, coexistence, and phasing

**Where it lives.** Unlike today's `serde`-generic path, a script codec needs
`&FuncLib` (to resolve names↔ids, arity, and output names) and produces a
`Graph` directly — so it is **not** a plain `SerdeFormat` variant. Put the
`Graph ⇄ script` codec in `scenarium` (takes `&FuncLib`); layer the
**positions/view-state** concern in `darkroom`'s `Document` (keyed by the
script variable name per §5). Keep the existing data formats (bitcode for
exact/fast, data-Rhai/JSON/TOML) as the **robust fallback and migration
bridge** — the script format is additive, not a replacement, until proven.

**Determinism/security.** Same posture as Starlark and the deprecated engine:
sandboxed Rhai, op/depth limits, `eval` disabled, no IO/clock/RNG registered,
source positions threaded onto each created node so trace/validation errors
point at the user's line.

**Phasing.**
1. **Phase 1 — export (graph → script), graph canonical.** Pure codegen in
   topological order; lossless; no loader yet. Lets us eyeball real graphs as
   scripts and validate the format/metadata design. Round-trip test: codegen,
   then byte-compare a re-serialized data form after a manual reference load.
2. **Phase 2 — tracing loader (script → graph).** Register per-func proxies +
   `NodeRef` + `on_var`; reconstruct `bindings`/`subscriptions`/`subgraphs`.
   Add a property test: random graph → script → trace → graph is identical up
   to fresh `NodeId`s (compare structure keyed by variable name).
3. **Phase 3 — authoring conveniences.** Object-map named args, `.once()`
   modifiers, operator sugar (`set_fast_operators(false)`), and one-way
   control-flow import. Each clearly documented as non-round-tripping where it
   applies.

## 11. Risks / open questions

- **Net value vs the data table.** Readability/diffability/expressiveness vs
  identity/layout/event friction, name stability, and executing untrusted code.
  Phase 1 (export only) is cheap and de-risks this — build it, look at real
  graphs, then decide on the loader.
- **Metadata clutter.** Inline `#{ id, pos, func }` maps can drown the dataflow
  they were meant to clarify. Alternative: a sidecar (`foo.rhai` +
  `foo.layout.toml`) keyed by variable name. Decide before Phase 2.
- **Subgraph instances vs definitions.** A `fn` defines a composite; calling it
  must trace to a `Subgraph` *instance* node (not inline the interior) to match
  `NodeKind::Subgraph(SubgraphRef::Local)`. Linked-vs-local resolution
  (`subgraph.rs:50`) needs a naming/`use`-like convention.
- **Partial/invalid scripts.** A trace that half-builds a graph on error — do
  we surface a partial graph or reject? (Lean reject; `Graph::deserialize`
  already `check()`s.)
- **Big graphs.** Thousands of nodes as a script: codegen variable naming and
  trace performance under the op-limit cap.

## 12. Sources

Tracing/proxy graph construction: PyTorch FX
([README](https://github.com/pytorch/pytorch/blob/main/torch/fx/README.md),
[docs](https://docs.pytorch.org/docs/2.12/fx.html)), `torch.jit.trace` vs
`script` ([trace](https://docs.pytorch.org/docs/stable/generated/torch.jit.trace.html),
[tracing-vs-scripting](https://ppwwyyxx.com/blog/2022/TorchScript-Tracing-vs-Scripting/),
[data-dependent control flow](https://thomasjpfan.com/2025/03/pytorch-graphs-three-ways-data-dependent-control-flow/)),
JAX ([key concepts](https://docs.jax.dev/en/latest/key-concepts.html),
[tracing](https://docs.jax.dev/en/latest/tracing.html)), TF AutoGraph
([intro to graphs](https://www.tensorflow.org/guide/intro_to_graphs)),
Dask `delayed` ([docs](https://docs.dask.org/en/stable/delayed.html)).
Execute-to-build determinism: Starlark
([spec](https://raw.githubusercontent.com/bazelbuild/starlark/master/spec.md),
[Bazel language](https://bazel.build/rules/language)), Nix
([derivations](https://nix.dev/manual/nix/2.18/command-ref/new-cli/nix3-derivation-show)).
Round-trip / source-of-truth: Unreal T3D clipboard
([Epic](https://x.com/unrealalexander/status/943621389524897792),
[forums](https://forums.unrealengine.com/t/question-uasset-copy-and-t3d/277319)),
Blender geometry-node copy limitation
([Blender Artists](https://blenderartists.org/t/i-cant-append-geometry-nodes-from-blend-files/1488168)).
Rhai APIs: [custom types](https://rhai.rs/book/rust/custom-types.html),
[getters/setters](https://rhai.rs/book/rust/getters-setters.html),
[indexers](https://rhai.rs/book/rust/indexers.html),
[register functions](https://rhai.rs/book/rust/functions.html),
[variable resolver `on_var`](https://rhai.rs/book/engine/var.html),
[operator overloading](https://rhai.rs/book/rust/operators.html),
[max call stack / safety](https://rhai.rs/book/safety/max-call-stack.html),
[Engine API](https://docs.rs/rhai/latest/rhai/struct.Engine.html).
In-repo: `common/src/serde_rhai/mod.rs` (sandboxed data-Rhai),
`darkroom-egui-deprecared/src/script/{mod.rs,prelude.rhai}` (executed-Rhai
graph mutation precedent).
