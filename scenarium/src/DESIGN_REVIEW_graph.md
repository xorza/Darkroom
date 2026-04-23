# Design review: scenarium/src/graph.rs  (2026-04-23)

## Current design

`Graph` is a flat container of `Node`s keyed by `NodeId`, stored in a
`KeyIndexVec<NodeId, Node>` (a `Vec` with a side-table for O(1) key
lookup). A `Node` owns its `inputs: Vec<Input>` (with per-port
`Binding` — `None | Const(StaticValue) | Bind(PortAddress)`) and
`events: Vec<Event>` (each event a name + `Vec<NodeId>` subscriber
list). The representation is the authored graph — no execution state,
no computed caches; every structural change goes through a small API
(`add`, `remove_by_id`, `by_id_mut`) or directly through the `pub
nodes` field.

Load-bearing decisions: (1) the `nodes` field is public, so every
caller traverses it directly — `graph.nodes.iter()`, `graph.nodes.len()`,
`graph.nodes.add(...)`; (2) `remove_by_id` eagerly rewrites dangling
state — any `Binding::Bind` pointing at the removed node is reset to
`Binding::None`, and the removed id is stripped from every event's
subscribers list (graph.rs:72–93). This keeps the invariant "no
dangling `Bind`s" without needing a validation sweep, at O(N·d) per
removal; (3) `dependent_nodes` computes the reverse-reachable set from
a given node by repeated linear scans, then returns hits in
`nodes.iter()` insertion order — that order is explicitly asserted by
a test (graph.rs:415–419) and also consumed by `connections.rs:279`
for connect-time cycle rejection.

## Overall take

The core model is right for an authored graph: plain data, no derived
state, `Bind` carries `NodeId` (stable) rather than an index (which
would shift on `remove`). The issues are smaller — a test fixture
that leaks into the release build, a container type exposed at the
field level, and one algorithmic choice whose cost is tolerable only
because graphs are currently small. No redesign warranted.

## Findings

### [F1] `test_graph` and `test_func_lib` are production API, not test fixtures

- **Category**: Responsibility / layering
- **Impact**: 3/5 — test fixtures shouldn't live in release binaries or show up in downstream crates' autocomplete; this is also the only reason `graph.rs` depends on `elements/basic_funclib`
- **Effort**: 2/5 — feature-gate in the crate, re-export from tests; ~4 test call sites don't need to change if the gate is `default = ["test-fixtures"]`
- **Current**: `pub fn test_graph()` at graph.rs:307 is an unconditional public function that builds a fixed 5-node demo graph with hardcoded UUIDs. It's re-exported via `prelude` (lib.rs:25–27). Line 7 of `graph.rs` reads `use crate::prelude::{TestFuncHooks, test_func_lib}` — a *production* module importing test fixtures out of its own prelude. Similarly `TestFuncHooks`, `test_func_lib`, and everything in `elements/basic_funclib.rs` (725 LOC of fixture functions) ship as public API.
- **Problem**: (a) binary bloat — `basic_funclib`'s 725 LOC + 5 hardcoded UUIDs are in every release build that links `scenarium`. (b) layering inversion — `graph.rs` pulling from `prelude` creates a cycle-in-intent: the prelude is supposed to *re-export* from modules, not feed back into them. (c) API pollution: downstream consumers (prism) see `test_graph` in autocomplete and may use it outside tests. (d) the test that asserts exact dependent ordering (graph.rs:415–419) is keyed to these specific UUIDs, baking fixture identity into the API surface.
- **Alternative**: Feature-gate — `#[cfg(any(test, feature = "test-fixtures"))]` on `test_graph`, `TestFuncHooks`, `test_func_lib`, and `basic_funclib`. The 4 test call sites in prism enable the feature via `scenarium = { path = ..., features = ["test-fixtures"] }` in `[dev-dependencies]`. Release builds drop the fixture functions and their LUT. Bonus: `graph.rs` no longer imports from its own prelude.
- **Recommendation**: Do it. The CLAUDE.md "Never use `#[cfg(test)]` on functions in production code" rule *permits* keeping test helpers out of production via feature flags, and this is the cleanest way to honor it.

### [F2] `pub nodes: KeyIndexVec<NodeId, Node>` leaks the container type at the field level

- **Category**: Abstraction / contract
- **Impact**: 3/5 — the container type shows up at ~15 call sites across prism and scenarium; changing it (or its iteration-order contract) requires touching all of them
- **Effort**: 3/5 — wrap with `len()`/`iter()`/`iter_mut()` methods on `Graph`, then update callers
- **Current**: `pub nodes: KeyIndexVec<NodeId, Node>` at graph.rs:65. Callers go straight through the field: `view_graph.graph.nodes.iter()` (view_graph.rs:71, 142; action_stack.rs:290, 336, 379, 411, 559; graph_ui_action.rs:102), `.nodes.len()` (view_graph.rs:141, 199–200), and more. `Graph::add` exists as a wrapper around `self.nodes.add(node)` (graph.rs:69–71) — the indirection already exists for *adding* but not for *reading*.
- **Problem**: Every reader now has a durable dependency on `KeyIndexVec<NodeId, Node>`. A hypothetical switch to, say, `HashMap<NodeId, Node>` + a sidecar `Vec<NodeId>` for ordering (to preserve the `dependent_nodes` order contract) would require editing all ~15 sites. More subtly, the iteration-order contract — which matters for `dependent_nodes` and for the default node-rendering order in `prism` — isn't documented on the public API; it's an artifact of `KeyIndexVec`'s impl.
- **Alternative**: Make `nodes` private; add methods:
  ```rust
  pub fn iter(&self) -> impl Iterator<Item = &Node> { self.nodes.iter() }
  pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Node> { self.nodes.iter_mut() }
  pub fn len(&self) -> usize { self.nodes.len() }
  pub fn is_empty(&self) -> bool { self.nodes.is_empty() }
  ```
  Document "iteration preserves insertion order" on `iter()`. Callers that reach in for `.nodes.iter()` become `.iter()`, `.nodes.len()` becomes `.len()`. The call-site pattern `view_graph.graph.nodes.iter()` → `view_graph.graph.iter()` is strictly shorter.
- **Recommendation**: Do it. The iteration-order contract is load-bearing enough that it deserves to be a method-level doc comment, not a transitive property of the field type.

### [F3] `dependent_nodes` solves reverse-adjacency ad-hoc; same pattern duplicated elsewhere

- **Category**: Responsibility
- **Impact**: 2/5 — at current graph sizes (tens of nodes) the O(N²) cost is invisible; the smell is that the *same* reverse-adjacency walk is reimplemented in `execution_graph::invalidate_recursively` (execution_graph/mod.rs:352) with the same O(N²·d) shape
- **Effort**: 2/5 — ~30 lines if consolidated into a single helper; more if made incremental
- **Current**: `dependent_nodes` (graph.rs:113–144) walks: start from a node, for each popped id, linear-scan *all* N nodes checking every input's binding. Collect a `seen` set, then iterate `self.nodes` again in insertion order to produce ordered output. Total: O(N²·d) where d = avg inputs per node. Same algorithmic shape in `execution_graph::invalidate_recursively`.
- **Problem**: Two independent O(N²) reverse-adjacency walks mean two places to change when the data model moves — and they're already subtly different (`dependent_nodes` returns ordered; `invalidate_recursively` doesn't). Consolidating forces the contract question: do consumers need insertion order, or just any topological order?
- **Alternative(s)**:
  1. **Do nothing** — N is small, the divergence is minor, the test locks the behavior. Cost: two implementations drift over time.
  2. **Shared helper**: extract `pub fn dependents_of(&self, node_id: NodeId) -> impl Iterator<Item = NodeId>` that yields in insertion order; `invalidate_recursively` consumes via `.collect()`. Still O(N²) per seed.
  3. **Precomputed reverse adjacency**: on `add`/`remove_by_id`/binding-change, maintain `reverse: HashMap<NodeId, SmallVec<NodeId>>`. O(N+E) queries. Overkill for current graph sizes; pays off if we ever hit hundreds of nodes.
- **Recommendation**: Option 2 if you touch this area for another reason. Not worth a dedicated pass — the two implementations are close enough that keeping them separate isn't actively harmful, and the N² cost doesn't matter here.

## Considered and rejected

- **`Node::default()` calling `NodeId::unique()`** (graph.rs:195–206) — a non-deterministic `Default` impl is a textbook footgun, but `rg Node::default\(\) scenarium/src prism/src` returns *zero* call sites. It only exists because `KeyIndexVec` requires `Default` bounds somewhere. Cost of removing: 0 callers break; cost of keeping: one day someone writes `Node::default()` in a test and gets a fresh UUID each time. Toss-up — not worth touching until someone actually trips on it.
- **`Binding::is_some()` / `Binding::is_none()` naming** (graph.rs:250–256) — the `Option`-parallel naming is fine; "has a binding" vs "no binding" reads clearly. One caller each; not leaky enough to rename.
- **`Binding::From` impl explosion** (graph.rs:265–296) — `From<PortAddress>`, `From<(NodeId, usize)>`, `From<&DataType>`, `From<StaticValue>`, `From<i64>`. The `i64` one is arbitrary (why not `u64`, `f64`, `bool`?) but all are used only in test fixtures. Keep as-is; consolidating to `impl<T: Into<StaticValue>> From<T> for Binding` conflicts with the `PortAddress` impl.
- **`assert!(!id.is_nil())` / `assert!(!name.is_empty())` on lookups** (graph.rs:95, 100, 104, 109, 114) — these guard against programmer errors (passing a default-constructed id into a lookup). Matches the project's "assert inputs to catch logic errors" rule. Leave.
- **Separate `validate` / `validate_with(&FuncLib)`** — the split is intentional: `validate` runs without a func lib (used in deserialize), `validate_with` adds cross-crate checks. Correct shape.
