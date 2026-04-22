# Design review: prism/src/gui/node_ui.rs  (2026-04-22)

## Current design

Two passes over `view_graph.view_nodes`, each iterating on demand: `handle_node_interactions` (line 154) hit-tests node bodies against a freshly-computed `NodeLayout`, accumulates drag deltas into `Gesture`, and emits `NodeSelected` / `NodeMoved` actions; `render_nodes` (line 216) does another pass, recomputing layouts against the now-updated gesture, and draws body / remove-btn / cache-btn / status / ports / labels. The two passes are sequenced by `graph_ui/mod.rs:167-218` around a `refresh_galleys` call. `NodeLayout` is produced on demand by `GraphLayout::node_layout` — no caching here.

Three public types leak outward: `PortInteractCommand` (routed to `connection_ui` for port-drag orchestration), `NodeExecutionInfo` (shared with `node_details_ui`), and `NodesFrameResult` (returned from `render_nodes` to the graph-ui orchestrator). `NodeUi` itself is a wrapper with one field (`const_bind_ui`), reached by `graph_ui/connections.rs:110` via `self.node_ui.const_bind_ui.broke_iter()`.

Invariants: interactions must run after `refresh_galleys` (documented at line 149-153; panic otherwise); the released-drag flag must be cleared at the top of each interaction pass (documented at line 162-168; flash bug otherwise). Both are documented, not type-enforced.

## Overall take

Structurally sound. The two-pass split (interact → render) is correct, the action-through-`FrameOutput` discipline is consistent, and rendering is decomposed into free functions with clear inputs. Remaining issues are boundary/layering and a handful of shape concerns on public types. No rewrite warranted.

## Findings

### [F1] `NodeExecutionInfo` doesn't belong in `node_ui`
- **Category**: Responsibility
- **Impact**: 3/5 — fixes a module boundary; removes a sibling-to-sibling import
- **Effort**: 2/5 — move type, repoint two imports
- **Current**: `NodeExecutionInfo` is defined at `node_ui.rs:82-125`. It's a classifier over `ExecutionStats` that picks which state applies to a given `NodeId`. Consumers: `node_ui::render_body` (for shadow selection) and `node_details_ui.rs:159` (for the side panel's execution readout, which uses the discriminant but not the `shadow()` method).
- **Problem**: `node_details_ui` imports from `node_ui` — a sibling-on-sibling dependency for a type that is *about* execution stats, not rendering. The `shadow(&Gui)` method is view-specific (only `render_body` needs it) but forces both consumers to depend on the same module.
- **Alternative**: Move the enum + `from_stats` to `model/execution_info.rs` (or similar under the model/stats layer — it's a pure projection). Keep `shadow()` as an inherent impl at the view layer, or drop it into `node_ui.rs` as a free function `fn exec_shadow(info: &NodeExecutionInfo, gui: &Gui)`. `node_details_ui` then pulls from the model layer alongside its existing `ExecutionStats` import.
- **Recommendation**: Do it.

### [F2] `NodeUi` is a single-field wrapper
- **Category**: Abstraction
- **Impact**: 2/5 — removes dead type, clarifies ownership
- **Effort**: 2/5 — move one field, demethod two methods
- **Current**: `NodeUi { const_bind_ui: ConstBindUi }` at line 131-134. `handle_node_interactions` takes `&self` but never reads from it; `render_nodes` uses `self` only to reach `const_bind_ui`. Owned by `GraphUi` (graph_ui/mod.rs:76).
- **Problem**: The type has no invariants, no behavior that justifies a struct. It's a namespace pretending to be a type. `graph_ui/connections.rs:110` bypasses the abstraction entirely (`self.node_ui.const_bind_ui.broke_iter()`), confirming that callers treat `node_ui` as a container, not an object.
- **Alternative**: Hoist `const_bind_ui` to `GraphUi`. Turn `handle_node_interactions` and `render_nodes` into free functions in this file (consistent with the render helpers that are already free functions). Call sites become `node_ui::handle_interactions(...)` / `node_ui::render(...)`. Matches the file's existing style and eliminates the empty shell.
- **Recommendation**: Do it.

### [F3] `NodesFrameResult` bundles three unrelated channels
- **Category**: Data structures
- **Impact**: 3/5 — clarifies output shape; removes per-frame Vec allocations in the common case
- **Effort**: 2/5 — migrate two fields to `FrameOutput`, update call site
- **Current**: `render_nodes` returns `NodesFrameResult { port_cmd: PortInteractCommand, removed_nodes: Vec<NodeId>, broken_nodes: Vec<NodeId> }` (line 142). At `graph_ui/mod.rs:203-215`, `removed_nodes` is drained into `NodeRemoved` actions, `broken_nodes` is forwarded to `process_connections`, and `port_cmd` is forwarded separately.
- **Problem**: These three fields are produced and consumed independently — there's no semantic reason they travel together. `removed_nodes` and `broken_nodes` are `Vec<NodeId>` that are almost always 0 or 1 element (one remove-btn click, or N breaker hits per stroke), so the allocation is noise. Worse, `removed_nodes` is conceptually just another action that should flow through `FrameOutput`.
- **Alternative**:
  - **Option A** (recommended): `render_nodes` returns `PortInteractCommand` directly. Remove-intents are emitted as actions at the render site (using the same pattern that already exists for `NodeSelected`/`NodeMoved`). Broken-node IDs flow through a dedicated `FrameOutput::broken_nodes` Vec that other passes can read.
  - **Option B**: Keep the struct but use `SmallVec<[NodeId; 1]>` — stack-allocate the common case.
- **Recommendation**: Option A. `FrameOutput` already centralizes "things this pass wants to emit"; removed-nodes belongs there.

### [F4] `PortInteractCommand` priority is magic-numbered
- **Category**: Types
- **Impact**: 1/5 — pure readability
- **Effort**: 1/5 — reorder variants, derive `Ord`
- **Current**: Line 62-70 maps variants to u8 priorities (0, 5, 8, 10, 15) with unexplained gaps; `prefer` compares numerically.
- **Problem**: The numbers encode "which port event wins when multiple fire in one frame" — but that ordering is the documentation, hidden behind arbitrary constants.
- **Alternative**: Reorder the enum declaration from lowest to highest priority (`None`, `Hover`, `DragStart`, `DragStop`, `Click`). Derive `PartialOrd`/`Ord` — Rust orders by declaration order. `prefer` becomes `if other > *self { *self = other; }`. Intent is structural.
- **Recommendation**: Do it.

### [F5] Gesture/layout ordering contracts are runtime-only
- **Category**: Contract
- **Impact**: 2/5 — would catch a real class of regressions at compile time
- **Effort**: 4/5 — requires type-state or equivalent restructuring
- **Current**: Comments at line 149-153 and 162-168 document two runtime-order contracts: "run after refresh_galleys" (panic if violated) and "clear released flag first" (flash bug if violated). Both were sources of real bugs during the recent refactor.
- **Problem**: A future refactor could silently reintroduce either bug. No compile-time enforcement.
- **Alternative**: Type-state on `GraphLayout` — `refresh_galleys` returns a `FreshGraphLayout<'a>` that `node_layout` requires. Analogous wrapper for gesture. Or: bundle both passes into a single `nodes_frame(...)` entry point that sequences them internally, removing the external contract entirely.
- **Recommendation**: Don't do it now. The call sequence is 20 lines in one function, the contracts are fresh in the code, and the effort dwarfs the benefit. Revisit if a second caller emerges.

## Considered and rejected

- **Merge interactions and render into a single pass.** Would simplify the contract in F5 but violates the z-order requirement (ports must register after bodies for click routing) and loses the no-lag property of "accumulate delta, then render with updated offset." The split is correct.
- **Cache `NodeLayout` between the two passes within a frame.** Each pass computes layouts with a different drag offset (pre-delta vs post-delta), so the second compute isn't redundant. Not a saving.
- **Replace `Vec<usize>` in `get_missing_input_ports` with a HashSet or caller-allocated buffer.** Per-frame allocation is genuine but tiny; most nodes have no missing inputs. Premature.
- **Remove `Clone` derive from `PortInteractCommand`.** Doesn't appear to be used (all transfers are by move). Would need grep-verification; not worth the review turn.
