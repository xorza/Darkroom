//! Forward-only descriptions of graph mutations + the self-contained
//! undo entries built from them. Ported from `darkroom-egui` —
//! same design rationale; substitute `glam::Vec2` for `egui::{Pos2,Vec2}`.
//!
//! An [`Intent`] is "what the caller wants the graph to look like
//! after"; it carries no history. To make the change reversible, we
//! pair the intent with a snapshot of the slot it overwrites. Rather
//! than carrying that snapshot in a sibling enum, [`UndoStep`] folds
//! both halves into one variant per kind: every variant has both the
//! "from" payload (for revert) and the "to" payload (for forward
//! apply). Type-level enforcement means an `UndoStep` can never be
//! constructed inconsistently — there's no `(Intent::A, Snapshot::B)`
//! mismatch to worry about at runtime.
//!
//! Three free fns own the per-variant logic:
//!   - [`build_step`] — read snapshot from `&Document`, fold with the
//!     incoming intent, return a fully-populated [`UndoStep`]. Pure.
//!   - [`apply_step`] — write the "to" half of an `UndoStep` to
//!     `&mut Document`. Used both during initial commit and during
//!     undo-stack redo.
//!   - [`revert_step`] — write the "from" half of an `UndoStep` to
//!     `&mut Document`. Used during undo.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::graph::{Binding, InputPort, Node, NodeBehavior, NodeId, Subscription};
use scenarium::prelude::{SubgraphDef, SubgraphId};
use serde::{Deserialize, Serialize};

use crate::document::view_node::ViewNode;
use crate::document::{BoundarySide, Document, EditScope, EditScopeRef, GraphRef};

/// What the caller wants to change. Forward-only — no `from` fields.
/// Each variant says "set X to Y"; the consumer captures the previous
/// Y at commit time via [`build_step`].
///
/// **Adding a variant** — touch these spots:
///   1. add the variant here on `Intent`,
///   2. add the matching variant on [`GraphStep`] (graph-scoped, edited
///      through an `EditScope`) or [`DocStep`] (document-global), carrying
///      both the forward "to" and backward "from" payloads (or just
///      forward fields for pure-creation intents),
///   3. add an arm to [`build_step`] (read `from` from `&Document`
///      and combine with the intent's `to` into a complete step),
///   4. add an arm to the matching `apply_*` / `revert_*` fn,
///   5. add arms to `is_noop` and [`requires_relayout`] (both
///      exhaustive — they won't compile until you do),
///   6. update [`gesture_key`] if the variant coalesces in undo
///      history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Intent {
    AddNode {
        view_node: ViewNode,
        node: Node,
        /// Local subgraph def to add alongside the node — set when the
        /// node is a `Subgraph(Local(_))` instance whose def the caller
        /// just created (e.g. instancing a library subgraph, which drops
        /// a localized copy). `None` for plain func nodes.
        def: Option<Box<SubgraphDef>>,
    },
    /// Paste a set of pre-cloned nodes (fresh ids, offset positions) plus
    /// the connections *among* them, and select the copies. The caller
    /// (Ctrl+D duplicate) builds the clones + remapped wiring; `build_step`
    /// only captures the prior selection. One undo entry for the whole
    /// duplicate.
    DuplicateNodes {
        nodes: Vec<(ViewNode, Node)>,
        bindings: Vec<(InputPort, Binding)>,
        subscriptions: Vec<Subscription>,
    },
    RemoveNode {
        node_id: NodeId,
    },
    /// Move one or more nodes. A multi-select drag moves the whole group
    /// as a single undo entry; a plain drag carries just the one node.
    /// `grabbed` is the node the pointer latched — it keys the drag
    /// gesture so consecutive frames coalesce. Each `to` entry is
    /// `(node_id, new_pos)`.
    MoveNodes {
        grabbed: NodeId,
        to: Vec<(NodeId, Vec2)>,
    },
    RenameNode {
        node_id: NodeId,
        to: String,
    },
    SetInput {
        node_id: NodeId,
        input_idx: usize,
        to: Binding,
    },
    /// Replace the whole selection set. The rubber band, node clicks,
    /// and Esc-deselect all funnel through this — the caller computes
    /// the desired final set and the undo layer captures the prior one.
    SetSelection {
        to: BTreeSet<NodeId>,
    },
    SetCacheBehavior {
        node_id: NodeId,
        to: NodeBehavior,
    },
    /// Add (`present = true`) or remove (`present = false`)
    /// `subscriber` from the event at `(event_node_id, event_idx)`.
    SetEventConnection {
        event_node_id: NodeId,
        event_idx: usize,
        subscriber: NodeId,
        present: bool,
    },
    SetViewport {
        pan: Vec2,
        scale: f32,
    },
    /// Make the tab at index `to` active. Document-global (not scoped to
    /// any one graph); undoable and coalescing so a flurry of switches
    /// collapses into a single history entry.
    SwitchTab {
        to: usize,
    },
    /// Close the tab at `index`. Document-global; undoable so a closed
    /// subgraph tab can be reopened with Ctrl+Z. The `Main` tab (index 0)
    /// is never closable — `build_step` drops the intent if it targets it
    /// or an out-of-range index.
    CloseTab {
        index: usize,
    },
    /// Rename a subgraph interface port (`def.inputs[idx]` for
    /// `side = Input`, `def.outputs[idx]` for `Output`). Scoped to the
    /// active `Local` target — `build_step` reads the `SubgraphId` from
    /// the drain `target`, so the intent only carries the side + index +
    /// new name. Dropped when the target isn't a subgraph interior.
    RenameBoundaryPort {
        side: BoundarySide,
        idx: usize,
        to: String,
    },
}

/// Self-contained undo-stack entry. Each leaf variant carries both
/// halves: the forward "to" payload (read by [`apply_step`]) and the
/// backward "from" payload (read by [`revert_step`]). Built from an
/// [`Intent`] via [`build_step`], which captures the pre-mutation state
/// from `&Document` at commit time.
///
/// Split by scope so apply/revert dispatch on the type: a [`GraphStep`]
/// is resolved against a `(graph, view)` `EditScope` for the batch's
/// target, while a [`DocStep`] mutates `Document` fields that live
/// outside any single graph. The graph path therefore can't even *name*
/// a document-global variant — no convention-only `unreachable!` arms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoStep {
    Graph(GraphStep),
    Doc(DocStep),
}

/// Steps applied through an [`EditScope`] (graph + view) for the batch's
/// `GraphRef` target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphStep {
    /// Pure creation: the "from" state is "node absent", which is
    /// implicit — undo removes the node by id (and `def` if present).
    /// `def` is a `Local` subgraph def added alongside the instance node
    /// (library-subgraph instancing); `None` for plain func nodes.
    AddNode {
        view_node: ViewNode,
        node: Node,
        def: Option<Box<SubgraphDef>>,
    },
    /// Add a batch of nodes + their internal wiring and swap the
    /// selection to the copies. Undo removes every added node (which
    /// cascade-drops the added bindings/subscriptions) and restores
    /// `from_selection`. `nodes` carry fresh ids, so there's no prior
    /// state to capture beyond the selection.
    DuplicateNodes {
        nodes: Vec<(ViewNode, Node)>,
        bindings: Vec<(InputPort, Binding)>,
        subscriptions: Vec<Subscription>,
        from_selection: BTreeSet<NodeId>,
        to_selection: BTreeSet<NodeId>,
    },
    /// Pre-removal state lives entirely on the step: every reference
    /// into the doomed node, so undo can fully restore it.
    RemoveNode {
        view_node: ViewNode,
        node: Node,
        /// All bindings `remove_by_id` drops (the node's own inputs + edges
        /// into it), captured so undo can fully restore the wiring.
        bindings: Vec<(InputPort, Binding)>,
        /// All subscriptions touching the node (as emitter or subscriber).
        subscriptions: Vec<Subscription>,
        was_selected: bool,
    },
    MoveNodes {
        grabbed: NodeId,
        /// `(node_id, from, to)` per moved node. Nodes missing at build
        /// time are dropped, so this can be shorter than the intent's `to`.
        moves: Vec<(NodeId, Vec2, Vec2)>,
    },
    RenameNode {
        node_id: NodeId,
        from: String,
        to: String,
    },
    SetInput {
        node_id: NodeId,
        input_idx: usize,
        from: Binding,
        to: Binding,
    },
    SetSelection {
        from: BTreeSet<NodeId>,
        to: BTreeSet<NodeId>,
    },
    SetCacheBehavior {
        node_id: NodeId,
        from: NodeBehavior,
        to: NodeBehavior,
    },
    SetEventConnection {
        event_node_id: NodeId,
        event_idx: usize,
        subscriber: NodeId,
        was_present: bool,
        present: bool,
    },
    SetViewport {
        from_pan: Vec2,
        from_scale: f32,
        to_pan: Vec2,
        to_scale: f32,
    },
}

/// Document-global steps — they mutate fields that aren't scoped to a
/// single graph (active tab, the tab list, a subgraph's interface), so
/// they bypass the `EditScope` resolution entirely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocStep {
    SwitchTab {
        from: usize,
        to: usize,
    },
    /// Captures everything needed to reopen the closed tab: the removed
    /// `target` and `index` (re-inserted on revert) plus the active index
    /// before/after the close.
    CloseTab {
        index: usize,
        target: GraphRef,
        from_active: usize,
        to_active: usize,
    },
    /// `sub_id` is resolved at build time so apply/revert are
    /// self-contained (don't need the drain target). Carries both names.
    ///
    /// `idx` is only a *hint*: apply/revert resolve the slot by name
    /// (`from`/`to`) via [`Document::rename_boundary_port`], so undo/redo
    /// survive `reconcile_boundaries` compacting the interface — it
    /// renumbers indices but preserves names. If the slot was
    /// disconnected away entirely the name is gone and the step no-ops
    /// (can't restore a name on a port that no longer exists). Residual
    /// ambiguity only under duplicate names *and* compaction together —
    /// rare and user-created.
    RenameBoundaryPort {
        sub_id: SubgraphId,
        side: BoundarySide,
        idx: usize,
        from: String,
        to: String,
    },
}

/// 1e-4 is the threshold below which two pan/scale samples are
/// considered the same gesture — keeps idle pan/zoom from polluting
/// the undo stack with sub-pixel deltas.
const VIEWPORT_EPS: f32 = 1e-4;

impl UndoStep {
    /// True when applying this step would leave the document unchanged.
    /// Filtered out post-`build_step` so phantom entries (re-selecting
    /// the same node, dragging zero pixels) don't pollute the undo stack.
    pub fn is_noop(&self) -> bool {
        match self {
            UndoStep::Graph(g) => g.is_noop(),
            UndoStep::Doc(d) => d.is_noop(),
        }
    }
}

impl GraphStep {
    /// Generic from/to equality; viewport uses an epsilon to absorb idle
    /// jitter. `AddNode` / `RemoveNode` are never no-ops (the existence
    /// flip is the change).
    fn is_noop(&self) -> bool {
        match self {
            GraphStep::AddNode { .. } | GraphStep::RemoveNode { .. } => false,
            GraphStep::DuplicateNodes { nodes, .. } => nodes.is_empty(),
            GraphStep::MoveNodes { moves, .. } => moves.iter().all(|(_, from, to)| from == to),
            GraphStep::RenameNode { from, to, .. } => from == to,
            GraphStep::SetInput { from, to, .. } => from == to,
            GraphStep::SetSelection { from, to } => from == to,
            GraphStep::SetCacheBehavior { from, to, .. } => from == to,
            GraphStep::SetEventConnection {
                was_present,
                present,
                ..
            } => was_present == present,
            GraphStep::SetViewport {
                from_pan,
                from_scale,
                to_pan,
                to_scale,
            } => {
                (*from_pan - *to_pan).length_squared() < VIEWPORT_EPS * VIEWPORT_EPS
                    && (*from_scale - *to_scale).abs() < VIEWPORT_EPS
            }
        }
    }
}

impl DocStep {
    fn is_noop(&self) -> bool {
        match self {
            DocStep::SwitchTab { from, to } => from == to,
            // A close always removes a tab (build_step drops invalid
            // targets up front), so it's never a no-op.
            DocStep::CloseTab { .. } => false,
            DocStep::RenameBoundaryPort { from, to, .. } => from == to,
        }
    }
}

/// Read pre-mutation state from `doc` and fold it with `intent`
/// into a complete [`UndoStep`]. Pure — does not write to the graph.
/// Returns `None` when the intent targets a node that no longer exists
/// (e.g. a `RemoveNode`/`SetInput` whose anchor lingered one frame past a
/// `RemoveNode` applied earlier in the same frame). Callers should treat a
/// `None` result as "stale intent, drop it". (`MoveNodes` instead skips
/// vanished nodes individually rather than dropping the whole batch.)
pub fn build_step(intent: Intent, doc: &Document, target: GraphRef) -> Option<UndoStep> {
    // Document-global intents don't resolve a graph scope.
    if let Intent::SwitchTab { to } = intent {
        return Some(UndoStep::Doc(DocStep::SwitchTab {
            from: doc.active,
            to,
        }));
    }
    if let Intent::CloseTab { index } = intent {
        // `Main` (index 0) is never closable, and a stale index is dropped.
        if index == 0 || index >= doc.tabs.len() {
            return None;
        }
        let from_active = doc.active;
        // Mirror `apply`'s active recompute so the forward step is
        // self-contained: shift left if we closed left of the cursor,
        // then clamp into the post-removal range (len shrinks by one).
        let mut to_active = if from_active > index {
            from_active - 1
        } else {
            from_active
        };
        to_active = to_active.min(doc.tabs.len() - 2);
        return Some(UndoStep::Doc(DocStep::CloseTab {
            index,
            target: doc.tabs[index],
            from_active,
            to_active,
        }));
    }
    if let Intent::RenameBoundaryPort { side, idx, to } = intent {
        // Boundary ports only exist in a subgraph interior; the def is
        // the active `Local` target's. Drop the rename otherwise.
        let GraphRef::Local(sub_id) = target else {
            return None;
        };
        let from = doc.boundary_port_name(sub_id, side, idx)?.to_owned();
        return Some(UndoStep::Doc(DocStep::RenameBoundaryPort {
            sub_id,
            side,
            idx,
            from,
            to,
        }));
    }
    let EditScopeRef { graph, view } = doc.scope(target)?;
    let step = match intent {
        Intent::SwitchTab { .. } | Intent::CloseTab { .. } | Intent::RenameBoundaryPort { .. } => {
            unreachable!("document-global intents handled above")
        }
        Intent::AddNode {
            view_node,
            node,
            def,
        } => GraphStep::AddNode {
            view_node,
            node,
            def,
        },
        Intent::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
        } => {
            let to_selection = nodes.iter().map(|(_, node)| node.id).collect();
            GraphStep::DuplicateNodes {
                nodes,
                bindings,
                subscriptions,
                from_selection: view.selected_nodes.clone(),
                to_selection,
            }
        }
        Intent::RemoveNode { node_id } => {
            let view_node = view.view_nodes.by_key(&node_id)?.clone();
            let node = graph.by_id(&node_id)?.clone();
            let was_selected = view.selected_nodes.contains(&node_id);
            GraphStep::RemoveNode {
                view_node,
                node,
                bindings: graph.bindings_touching(node_id),
                subscriptions: graph.subscriptions_touching(node_id),
                was_selected,
            }
        }
        Intent::MoveNodes { grabbed, to } => {
            let mut moves = Vec::with_capacity(to.len());
            for (id, t) in to {
                if let Some(vn) = view.view_nodes.by_key(&id) {
                    moves.push((id, vn.pos, t));
                }
            }
            GraphStep::MoveNodes { grabbed, moves }
        }
        Intent::RenameNode { node_id, to } => GraphStep::RenameNode {
            from: graph.by_id(&node_id)?.name.clone(),
            node_id,
            to,
        },
        Intent::SetInput {
            node_id,
            input_idx,
            to,
        } => {
            graph.by_id(&node_id)?;
            GraphStep::SetInput {
                from: graph.input_binding(InputPort::new(node_id, input_idx)),
                node_id,
                input_idx,
                to,
            }
        }
        Intent::SetSelection { to } => GraphStep::SetSelection {
            from: view.selected_nodes.clone(),
            to,
        },
        Intent::SetCacheBehavior { node_id, to } => GraphStep::SetCacheBehavior {
            from: graph.by_id(&node_id)?.behavior,
            node_id,
            to,
        },
        Intent::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
        } => {
            graph.by_id(&event_node_id)?;
            GraphStep::SetEventConnection {
                was_present: graph.is_subscribed(event_node_id, event_idx, subscriber),
                event_node_id,
                event_idx,
                subscriber,
                present,
            }
        }
        Intent::SetViewport { pan, scale } => GraphStep::SetViewport {
            from_pan: view.pan,
            from_scale: view.scale,
            to_pan: pan,
            to_scale: scale,
        },
    };
    Some(UndoStep::Graph(step))
}

/// Resolve the right graph+view for a scoped step, run `body`, and
/// no-op if the target graph has since disappeared (a subgraph deleted
/// while its undo entries linger).
fn with_scope(doc: &mut Document, target: GraphRef, body: impl FnOnce(&mut EditScope<'_>)) {
    if let Some(mut scope) = doc.scope_mut(target) {
        body(&mut scope);
    }
}

/// Forward apply: write the step's "to" half to `doc`. Used by
/// the initial commit (right after `build_step`) and by undo-stack
/// redo (replaying a popped step).
pub fn apply_step(step: &UndoStep, doc: &mut Document, target: GraphRef) {
    match step {
        UndoStep::Doc(step) => apply_doc(step, doc),
        UndoStep::Graph(step) => with_scope(doc, target, |scope| apply_graph(step, scope)),
    }
}

/// Forward-apply a document-global step.
fn apply_doc(step: &DocStep, doc: &mut Document) {
    match step {
        DocStep::SwitchTab { to, .. } => doc.active = *to,
        DocStep::CloseTab {
            index, to_active, ..
        } => {
            doc.tabs.remove(*index);
            doc.active = *to_active;
        }
        DocStep::RenameBoundaryPort {
            sub_id,
            side,
            idx,
            from,
            to,
        } => doc.rename_boundary_port(*sub_id, *side, *idx, from, to),
    }
}

/// Forward-apply a graph-scoped step against its resolved `EditScope`.
fn apply_graph(step: &GraphStep, scope: &mut EditScope<'_>) {
    match step {
        GraphStep::AddNode {
            view_node,
            node,
            def,
        } => {
            assert!(
                scope.graph.by_id(&node.id).is_none(),
                "apply AddNode expects node to be absent"
            );
            if let Some(def) = def {
                scope.graph.subgraphs.add((**def).clone());
            }
            scope.graph.add(node.clone());
            scope.view.view_nodes.add(view_node.clone());
        }
        GraphStep::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
            to_selection,
            ..
        } => {
            for (view_node, node) in nodes {
                scope.graph.add(node.clone());
                scope.view.view_nodes.add(view_node.clone());
            }
            for (port, binding) in bindings {
                scope.graph.set_input_binding(*port, binding.clone());
            }
            for s in subscriptions {
                scope.graph.subscribe(s.emitter, s.event_idx, s.subscriber);
            }
            scope.view.selected_nodes = to_selection.clone();
        }
        GraphStep::RemoveNode { node, .. } => {
            assert!(
                scope.graph.by_id(&node.id).is_some(),
                "apply RemoveNode expects node to be present"
            );
            scope.remove_node(&node.id);
        }
        GraphStep::MoveNodes { moves, .. } => {
            for (id, _, to) in moves {
                if let Some(vn) = scope.view.view_nodes.by_key_mut(id) {
                    vn.pos = *to;
                }
            }
        }
        GraphStep::RenameNode { node_id, to, .. } => {
            scope.graph.by_id_mut(node_id).unwrap().name = to.clone();
        }
        GraphStep::SetInput {
            node_id,
            input_idx,
            to,
            ..
        } => {
            scope
                .graph
                .set_input_binding(InputPort::new(*node_id, *input_idx), to.clone());
        }
        GraphStep::SetSelection { to, .. } => {
            scope.view.selected_nodes = to.clone();
        }
        GraphStep::SetCacheBehavior { node_id, to, .. } => {
            scope.graph.by_id_mut(node_id).unwrap().behavior = *to;
        }
        GraphStep::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            present,
            ..
        } => {
            // subscribe/unsubscribe are idempotent (BTreeSet insert/remove),
            // so apply/redo is a no-op when the target state already holds.
            if *present {
                scope
                    .graph
                    .subscribe(*event_node_id, *event_idx, *subscriber);
            } else {
                scope
                    .graph
                    .unsubscribe(*event_node_id, *event_idx, *subscriber);
            }
        }
        GraphStep::SetViewport {
            to_pan, to_scale, ..
        } => {
            scope.view.pan = *to_pan;
            scope.view.scale = *to_scale;
        }
    }
}

/// Backward apply: write the step's "from" half to `doc`. Pairs
/// with [`apply_step`]; calling one after the other restores the
/// graph to its pre-commit state.
pub fn revert_step(step: &UndoStep, doc: &mut Document, target: GraphRef) {
    match step {
        UndoStep::Doc(step) => revert_doc(step, doc),
        UndoStep::Graph(step) => with_scope(doc, target, |scope| revert_graph(step, scope)),
    }
}

/// Backward-apply a document-global step.
fn revert_doc(step: &DocStep, doc: &mut Document) {
    match step {
        DocStep::SwitchTab { from, .. } => doc.active = *from,
        DocStep::CloseTab {
            index,
            target,
            from_active,
            ..
        } => {
            doc.tabs.insert(*index, *target);
            doc.active = *from_active;
        }
        DocStep::RenameBoundaryPort {
            sub_id,
            side,
            idx,
            from,
            to,
        } => doc.rename_boundary_port(*sub_id, *side, *idx, to, from),
    }
}

/// Backward-apply a graph-scoped step against its resolved `EditScope`.
fn revert_graph(step: &GraphStep, scope: &mut EditScope<'_>) {
    match step {
        GraphStep::AddNode { node, def, .. } => {
            scope.remove_node(&node.id);
            if let Some(def) = def {
                scope.graph.subgraphs.remove_by_key(&def.id);
            }
        }
        GraphStep::DuplicateNodes {
            nodes,
            from_selection,
            ..
        } => {
            // Removing each added node cascade-drops the bindings and
            // subscriptions that referenced it, so the batch's wiring goes
            // with it — only the selection needs explicit restoring.
            for (_, node) in nodes {
                scope.remove_node(&node.id);
            }
            scope.view.selected_nodes = from_selection.clone();
        }
        GraphStep::RemoveNode {
            view_node,
            node,
            bindings,
            subscriptions,
            was_selected,
        } => {
            let removed_node_id = node.id;
            assert!(
                scope.graph.by_id(&node.id).is_none(),
                "revert RemoveNode expects removed node to be absent"
            );
            scope.graph.add(node.clone());
            scope.view.view_nodes.add(view_node.clone());
            scope.graph.restore_wiring(bindings, subscriptions);
            if *was_selected {
                scope.view.selected_nodes.insert(removed_node_id);
            }
        }
        GraphStep::MoveNodes { moves, .. } => {
            for (id, from, _) in moves {
                if let Some(vn) = scope.view.view_nodes.by_key_mut(id) {
                    vn.pos = *from;
                }
            }
        }
        GraphStep::RenameNode { node_id, from, .. } => {
            scope.graph.by_id_mut(node_id).unwrap().name = from.clone();
        }
        GraphStep::SetInput {
            node_id,
            input_idx,
            from,
            ..
        } => {
            scope
                .graph
                .set_input_binding(InputPort::new(*node_id, *input_idx), from.clone());
        }
        GraphStep::SetSelection { from, .. } => {
            scope.view.selected_nodes = from.clone();
        }
        GraphStep::SetCacheBehavior { node_id, from, .. } => {
            scope.graph.by_id_mut(node_id).unwrap().behavior = *from;
        }
        GraphStep::SetEventConnection {
            event_node_id,
            event_idx,
            subscriber,
            was_present,
            ..
        } => {
            if *was_present {
                scope
                    .graph
                    .subscribe(*event_node_id, *event_idx, *subscriber);
            } else {
                scope
                    .graph
                    .unsubscribe(*event_node_id, *event_idx, *subscriber);
            }
        }
        GraphStep::SetViewport {
            from_pan,
            from_scale,
            ..
        } => {
            scope.view.pan = *from_pan;
            scope.view.scale = *from_scale;
        }
    }
}

/// Whether replaying this step changes anything the layout engine
/// reads (node positions, sizes, label text length, viewport
/// transform). When true, `App::frame` calls `ui.request_relayout()`
/// after applying the batch so the next pass picks up the change.
/// UI-only state with no measure/arrange input (selection, cache
/// behavior, model-only bindings) returns false. Exhaustive on
/// purpose — a new variant must declare its layout effect.
pub fn requires_relayout(step: &UndoStep) -> bool {
    match step {
        // Switching or closing a tab swaps which graph the scene renders
        // (and reshapes the tab strip); a port rename changes a label's
        // width so the node remeasures — all relayout the canvas.
        UndoStep::Doc(
            DocStep::SwitchTab { .. }
            | DocStep::CloseTab { .. }
            | DocStep::RenameBoundaryPort { .. },
        ) => true,
        UndoStep::Graph(g) => match g {
            GraphStep::AddNode { .. }
            | GraphStep::DuplicateNodes { .. }
            | GraphStep::RemoveNode { .. }
            | GraphStep::MoveNodes { .. }
            | GraphStep::RenameNode { .. } => true,
            // Viewport is the inner-canvas `TranslateScale`, applied at
            // paint; children arrange in pre-transform space, so a pan/zoom
            // changes nothing the layout engine reads — no Pass B needed.
            GraphStep::SetViewport { .. } => false,
            // The inline const-value editor is recorded only when the
            // binding is `Const(_)`. Flipping Const presence (None ⇄ Const,
            // Bind ⇄ Const) toggles the editor in the widget tree, so the
            // node remeasures and ports shift — connection curves must
            // re-sample their endpoints. Typing inside an existing `Const`
            // keeps the editor present at its `Fixed` size, so the
            // value-only edit (Const → Const) doesn't need a relayout.
            GraphStep::SetInput { from, to, .. } => {
                matches!(from, Binding::Const(_)) != matches!(to, Binding::Const(_))
            }
            GraphStep::SetSelection { .. }
            | GraphStep::SetCacheBehavior { .. }
            | GraphStep::SetEventConnection { .. } => false,
        },
    }
}

/// Identifies "same continuous gesture" for undo coalescing. The undo
/// stack collapses consecutive steps with the same key into one entry
/// (keeping the *first* "from" payload). Two viewport changes coalesce;
/// two `MoveNodes` of the *same* grabbed node coalesce.
pub fn gesture_key(step: &UndoStep) -> Option<GestureKey> {
    match step {
        UndoStep::Graph(GraphStep::SetViewport { .. }) => Some(GestureKey::Viewport),
        UndoStep::Graph(GraphStep::MoveNodes { grabbed, .. }) => {
            Some(GestureKey::NodeDrag(*grabbed))
        }
        // Consecutive tab switches collapse into one entry: the merged
        // step keeps the original `from` and adopts the latest `to`, so
        // a burst of tabbing undoes back to where it started in one step.
        UndoStep::Doc(DocStep::SwitchTab { .. }) => Some(GestureKey::TabSwitch),
        // Everything else is its own undo entry — e.g. closing two tabs
        // in a row shouldn't collapse into one Ctrl+Z.
        UndoStep::Graph(
            GraphStep::AddNode { .. }
            | GraphStep::DuplicateNodes { .. }
            | GraphStep::RemoveNode { .. }
            | GraphStep::RenameNode { .. }
            | GraphStep::SetInput { .. }
            | GraphStep::SetSelection { .. }
            | GraphStep::SetCacheBehavior { .. }
            | GraphStep::SetEventConnection { .. },
        )
        | UndoStep::Doc(DocStep::CloseTab { .. } | DocStep::RenameBoundaryPort { .. }) => None,
    }
}

/// Fold two consecutive steps of the same gesture into one: keep `prev`'s
/// "from" half and adopt `next`'s "to" half. `None` for any pair that
/// doesn't coalesce. The undo stack calls this after matching
/// [`gesture_key`] (so the pair is the same variant, and for `NodeDrag`
/// the same node), but the match below re-checks the pairing so the fold
/// stays self-contained — variant internals live here next to the step
/// definitions, not in the stack. Keep this in sync with `gesture_key`.
pub fn coalesce(prev: &UndoStep, next: &UndoStep) -> Option<UndoStep> {
    match (prev, next) {
        (
            UndoStep::Graph(GraphStep::SetViewport {
                from_pan,
                from_scale,
                ..
            }),
            UndoStep::Graph(GraphStep::SetViewport {
                to_pan, to_scale, ..
            }),
        ) => Some(UndoStep::Graph(GraphStep::SetViewport {
            from_pan: *from_pan,
            from_scale: *from_scale,
            to_pan: *to_pan,
            to_scale: *to_scale,
        })),
        (
            UndoStep::Graph(GraphStep::MoveNodes {
                grabbed,
                moves: prev,
            }),
            UndoStep::Graph(GraphStep::MoveNodes { moves: next, .. }),
        ) => {
            // Same gesture (matched `NodeDrag` key) ⇒ same node set; keep
            // each node's original `from`, adopt its latest `to`.
            let merged = prev
                .iter()
                .map(|(id, from, prev_to)| {
                    let to = next
                        .iter()
                        .find(|(nid, _, _)| nid == id)
                        .map(|(_, _, t)| *t)
                        .unwrap_or(*prev_to);
                    (*id, *from, to)
                })
                .collect();
            Some(UndoStep::Graph(GraphStep::MoveNodes {
                grabbed: *grabbed,
                moves: merged,
            }))
        }
        (
            UndoStep::Doc(DocStep::SwitchTab { from, .. }),
            UndoStep::Doc(DocStep::SwitchTab { to, .. }),
        ) => Some(UndoStep::Doc(DocStep::SwitchTab {
            from: *from,
            to: *to,
        })),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKey {
    Viewport,
    NodeDrag(NodeId),
    TabSwitch,
}
