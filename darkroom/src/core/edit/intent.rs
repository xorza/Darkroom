//! Forward-only descriptions of graph mutations + the self-contained
//! undo entries built from them.
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
//!
//! [`commit_intent`] is the high-level entry the live frontends drive
//! their per-intent loop through: build → no-op-filter → apply in one
//! call, returning the committed step to record. The two halves stay
//! public for undo-stack redo, which applies a *stored* step without
//! rebuilding it.

use std::collections::{BTreeSet, HashMap};

use glam::Vec2;
use scenarium::graph::{
    Binding, Graph, InputPort, Node, NodeBehavior, NodeId, NodeKind, OutputPort, Subscription,
};
use scenarium::prelude::{SubgraphDef, SubgraphId};
use scenarium::subgraph::SubgraphRef;
use serde::{Deserialize, Serialize};

use crate::core::document::view_node::ViewNode;
use crate::core::document::{BoundarySide, Document, EditScope, EditScopeRef, GraphRef};

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
///   5. add arms to [`UndoStep::is_noop`] and
///      [`UndoStep::requires_relayout`] (both exhaustive — they won't
///      compile until you do),
///   6. update [`UndoStep::gesture_key`] if the variant coalesces in undo
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
        /// Initial input bindings to seed alongside the node — the caller
        /// fills these with each input's func-declared default
        /// (`Binding::Const`) so a fresh node lands ready to run instead of
        /// fully unbound. Applied atomically with the node, so one undo
        /// removes node + seeds together.
        bindings: Vec<(InputPort, Binding)>,
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
    /// Enable/disable a node for execution (`Node::disabled`). The header
    /// badge toggles it; a disabled node is skipped at flatten time.
    SetDisabled {
        node_id: NodeId,
        to: bool,
    },
    /// Fork a private standalone copy of a `Subgraph(Local(_))` node's
    /// def and re-point the node at it (the S-badge "Detach" action).
    /// The copy gets fresh ids and a cleared `origin`, so it diverges
    /// from any sibling instances *and* from the library it came from.
    /// `build_step` reads the source def from the active graph; dropped
    /// when the node isn't a local subgraph instance.
    DetachSubgraph {
        node_id: NodeId,
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
    /// Rename a local subgraph def (`graph.subgraphs[id].name`).
    /// Document-global (not scoped to any one graph) so it works
    /// regardless of which tab is active. Drives the tab-strip's
    /// double-click-to-rename label.
    RenameSubgraph {
        id: SubgraphId,
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
        bindings: Vec<(InputPort, Binding)>,
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
    SetDisabled {
        node_id: NodeId,
        from: bool,
        to: bool,
    },
    /// Fork + re-point: `def` (a fresh standalone copy) joins the
    /// graph's local defs and the node swaps from `from_id` to `def.id`.
    /// Undo restores the `from_id` ref and drops `def`.
    DetachSubgraph {
        node_id: NodeId,
        from_id: SubgraphId,
        def: Box<SubgraphDef>,
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
    /// Rename a local subgraph def. Self-contained on the step (both
    /// names) so apply/revert don't need to re-read the doc.
    RenameSubgraph {
        id: SubgraphId,
        from: String,
        to: String,
    },
}

/// 1e-4 is the threshold below which two pan/scale samples are
/// considered the same gesture — keeps idle pan/zoom from polluting
/// the undo stack with sub-pixel deltas.
const VIEWPORT_EPS: f32 = 1e-4;

/// World-space offset applied to duplicated nodes so the copies don't
/// land exactly on top of their originals.
const DUPLICATE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

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
            GraphStep::AddNode { .. }
            | GraphStep::RemoveNode { .. }
            | GraphStep::DetachSubgraph { .. } => false,
            GraphStep::DuplicateNodes { nodes, .. } => nodes.is_empty(),
            GraphStep::MoveNodes { moves, .. } => moves.iter().all(|(_, from, to)| from == to),
            GraphStep::RenameNode { from, to, .. } => from == to,
            GraphStep::SetInput { from, to, .. } => from == to,
            GraphStep::SetSelection { from, to } => from == to,
            GraphStep::SetCacheBehavior { from, to, .. } => from == to,
            GraphStep::SetDisabled { from, to, .. } => from == to,
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
            DocStep::RenameSubgraph { from, to, .. } => from == to,
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
    if let Intent::RenameSubgraph { id, to } = intent {
        let from = doc.graph.subgraphs.by_key(&id)?.name.clone();
        return Some(UndoStep::Doc(DocStep::RenameSubgraph { id, from, to }));
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
        Intent::SwitchTab { .. }
        | Intent::CloseTab { .. }
        | Intent::RenameBoundaryPort { .. }
        | Intent::RenameSubgraph { .. } => {
            unreachable!("document-global intents handled above")
        }
        Intent::AddNode {
            view_node,
            mut node,
            def,
            bindings,
        } => {
            let def = reuse_local_subgraph(graph, &mut node, def);
            GraphStep::AddNode {
                view_node,
                node,
                def,
                bindings,
            }
        }
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
        Intent::SetDisabled { node_id, to } => GraphStep::SetDisabled {
            from: graph.by_id(&node_id)?.disabled,
            node_id,
            to,
        },
        Intent::DetachSubgraph { node_id } => {
            let NodeKind::Subgraph(SubgraphRef::Local(from_id)) = graph.by_id(&node_id)?.kind
            else {
                return None; // not a local subgraph instance — nothing to fork
            };
            let mut copy = graph.subgraphs.by_key(&from_id)?.fresh_copy();
            copy.origin = None; // detach severs the library lineage
            GraphStep::DetachSubgraph {
                node_id,
                from_id,
                def: Box::new(copy),
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

/// Build, no-op-filter, and apply one `intent` against `target` in a
/// single call — the high-level entry both live frontends (the GUI
/// [`crate::gui::app::editor::Editor`] and the headless
/// [`crate::core::session::Session`]) drive their per-intent loop through.
///
/// Returns the committed [`UndoStep`] (the caller records it and reads its
/// `requires_*` signals), or `None` when the intent was stale (anchor node
/// gone) or a no-op — in both cases nothing was written. [`build_step`] /
/// [`apply_step`] stay separate for the undo-stack redo path, which applies
/// a stored step without rebuilding it.
pub fn commit_intent(intent: Intent, doc: &mut Document, target: GraphRef) -> Option<UndoStep> {
    let step = build_step(intent, doc, target)?;
    if step.is_noop() {
        return None;
    }
    apply_step(&step, doc, target);
    Some(step)
}

/// Build an [`Intent::DuplicateNodes`] for `target`'s current selection.
/// Thin wrapper over [`build_duplicate_intent_for`] with the selection as
/// the node set and incoming (external) wires dropped — the Ctrl+D path.
pub fn build_duplicate_intent(doc: &Document, target: GraphRef) -> Option<Intent> {
    let EditScopeRef { view, .. } = doc.scope(target)?;
    if view.selected_nodes.is_empty() {
        return None;
    }
    build_duplicate_intent_for(doc, target, &view.selected_nodes, false)
}

/// Build an [`Intent::DuplicateNodes`] cloning `node_ids` in `target`: each
/// node gets a fresh id and an offset position, const-value bindings copy
/// verbatim, and the data + event connections *among* `node_ids` are
/// recreated against the clones. A `Bind` whose source is *outside* the set
/// is dropped unless `include_incoming` is set, in which case the clone
/// keeps the wire pointing at the original external producer. `None` when
/// `node_ids` is empty or the target doesn't resolve. Reads the document to
/// assemble the intent — editor-operation construction, kept with the rest
/// of the intent machinery rather than on the `Document` model.
pub fn build_duplicate_intent_for(
    doc: &Document,
    target: GraphRef,
    node_ids: &BTreeSet<NodeId>,
    include_incoming: bool,
) -> Option<Intent> {
    let EditScopeRef { graph, view } = doc.scope(target)?;
    if node_ids.is_empty() {
        return None;
    }

    let mut id_map: HashMap<NodeId, NodeId> = HashMap::new();
    let mut nodes = Vec::new();
    for old_id in node_ids {
        let Some(node) = graph.by_id(old_id) else {
            continue;
        };
        let new_id = NodeId::unique();
        id_map.insert(*old_id, new_id);
        let mut clone = node.clone();
        clone.id = new_id;
        let pos = view
            .view_nodes
            .by_key(old_id)
            .expect("view holds a position for every graph node")
            .pos
            + DUPLICATE_OFFSET;
        nodes.push((ViewNode { id: new_id, pos }, clone));
    }

    // Each cloned node's own input ports. Const/None copy verbatim; a `Bind`
    // to a source inside the set is remapped to that source's clone. A `Bind`
    // to an *external* source is dropped — unless `include_incoming`, where
    // the clone keeps the wire to the original producer.
    let mut bindings = Vec::new();
    for old_id in node_ids {
        for (port, binding) in graph.bindings_touching(*old_id) {
            if port.node_id != *old_id {
                continue;
            }
            let new_binding = match binding {
                Binding::Bind(src) => match id_map.get(&src.node_id) {
                    Some(&new_src) => Binding::Bind(OutputPort {
                        node_id: new_src,
                        port_idx: src.port_idx,
                    }),
                    None if include_incoming => Binding::Bind(src),
                    None => continue,
                },
                other => other,
            };
            bindings.push((InputPort::new(id_map[old_id], port.port_idx), new_binding));
        }
    }

    // Event subscriptions internal to the set.
    let mut subscriptions = Vec::new();
    for s in graph.subscriptions() {
        if let (Some(&emitter), Some(&subscriber)) =
            (id_map.get(&s.emitter), id_map.get(&s.subscriber))
        {
            subscriptions.push(Subscription {
                emitter,
                event_idx: s.event_idx,
                subscriber,
            });
        }
    }

    Some(Intent::DuplicateNodes {
        nodes,
        bindings,
        subscriptions,
    })
}

/// Library subgraphs are localized on instance: the new-node menu drops
/// a fresh `Local` copy tagged with the library def it came from
/// (`origin`). If `graph` already holds a local def from the same
/// library source, re-point `node` at that existing def and drop the
/// duplicate copy — one editable local def, many instances. Otherwise
/// return `def` untouched (the first instance materializes the copy).
fn reuse_local_subgraph(
    graph: &Graph,
    node: &mut Node,
    def: Option<Box<SubgraphDef>>,
) -> Option<Box<SubgraphDef>> {
    let def = def?;
    // No library lineage (hand-authored local def, e.g. collapse-to-
    // subgraph) → always its own copy; nothing to dedup against.
    let Some(origin) = def.origin else {
        return Some(def);
    };
    match graph.subgraphs.iter().find(|d| d.origin == Some(origin)) {
        Some(existing) => {
            node.kind = NodeKind::Subgraph(SubgraphRef::Local(existing.id));
            None
        }
        None => Some(def),
    }
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
        DocStep::RenameSubgraph { id, to, .. } => {
            if let Some(def) = doc.graph.subgraphs.by_key_mut(id) {
                def.name = to.clone();
            }
        }
    }
}

/// Forward-apply a graph-scoped step against its resolved `EditScope`.
fn apply_graph(step: &GraphStep, scope: &mut EditScope<'_>) {
    match step {
        GraphStep::AddNode {
            view_node,
            node,
            def,
            bindings,
        } => {
            assert!(
                scope.graph.by_id(&node.id).is_none(),
                "apply AddNode expects node to be absent"
            );
            if let Some(def) = def {
                scope.graph.subgraphs.add((**def).clone());
            }
            scope.graph.add(node.clone());
            for (port, binding) in bindings {
                scope.graph.set_input_binding(*port, binding.clone());
            }
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
        GraphStep::SetDisabled { node_id, to, .. } => {
            scope.graph.by_id_mut(node_id).unwrap().disabled = *to;
        }
        GraphStep::DetachSubgraph { node_id, def, .. } => {
            scope.graph.subgraphs.add((**def).clone());
            scope.graph.by_id_mut(node_id).unwrap().kind =
                NodeKind::Subgraph(SubgraphRef::Local(def.id));
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
        DocStep::RenameSubgraph { id, from, .. } => {
            if let Some(def) = doc.graph.subgraphs.by_key_mut(id) {
                def.name = from.clone();
            }
        }
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
        GraphStep::SetDisabled { node_id, from, .. } => {
            scope.graph.by_id_mut(node_id).unwrap().disabled = *from;
        }
        GraphStep::DetachSubgraph {
            node_id,
            from_id,
            def,
        } => {
            scope.graph.by_id_mut(node_id).unwrap().kind =
                NodeKind::Subgraph(SubgraphRef::Local(*from_id));
            scope.graph.subgraphs.remove_by_key(&def.id);
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

impl UndoStep {
    /// Whether replaying this step changes anything the layout engine
    /// reads (node positions, sizes, label text length, viewport
    /// transform). When true, `App::frame` calls `ui.request_relayout()`
    /// after applying the batch so the next pass picks up the change.
    /// UI-only state with no measure/arrange input (selection, cache
    /// behavior, model-only bindings) returns false. Exhaustive on
    /// purpose — a new variant must declare its layout effect.
    pub fn requires_relayout(&self) -> bool {
        match self {
        // Switching or closing a tab swaps which graph the scene renders
        // (and reshapes the tab strip); a port rename changes a label's
        // width so the node remeasures — all relayout the canvas.
        UndoStep::Doc(
            DocStep::SwitchTab { .. }
            | DocStep::CloseTab { .. }
            | DocStep::RenameBoundaryPort { .. }
            // Subgraph rename changes the tab-strip label's width.
            | DocStep::RenameSubgraph { .. },
        ) => true,
        UndoStep::Graph(g) => match g {
            GraphStep::AddNode { .. }
            | GraphStep::DuplicateNodes { .. }
            | GraphStep::RemoveNode { .. }
            | GraphStep::MoveNodes { .. }
            | GraphStep::RenameNode { .. }
            // Forks an identical-interface def, so the node doesn't
            // resize — but it's a structural edit and rare, so eat one
            // relayout rather than reason about it staying in lockstep.
            | GraphStep::DetachSubgraph { .. } => true,
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
            // Disabling only dims the body paint — same rect, no remeasure.
            | GraphStep::SetDisabled { .. } => false,
        },
    }
    }

    /// Whether applying this step can change a subgraph's *derived interface*
    /// (`def.inputs`/`def.outputs`), so `reconcile_boundaries` must rerun
    /// before the next scene rebuild. Only interior boundary wiring and
    /// instance bindings feed that derivation, so any edit that touches a
    /// binding or the node set qualifies; pure view/selection/cache/tab edits
    /// (and boundary-port *renames*, which reconcile preserves) never do.
    /// Conservative on `SetInput` — a const-value edit on a plain func port
    /// can't change an interface, but filtering that needs a doc lookup, and
    /// reconcile is an idempotent no-op there anyway. Exhaustive on purpose.
    pub fn requires_reconcile(&self) -> bool {
        match self {
            UndoStep::Graph(
                GraphStep::AddNode { .. }
                | GraphStep::RemoveNode { .. }
                | GraphStep::DuplicateNodes { .. }
                | GraphStep::SetInput { .. }
                | GraphStep::DetachSubgraph { .. },
            ) => true,
            UndoStep::Graph(
                GraphStep::MoveNodes { .. }
                | GraphStep::RenameNode { .. }
                | GraphStep::SetSelection { .. }
                | GraphStep::SetCacheBehavior { .. }
                | GraphStep::SetDisabled { .. }
                | GraphStep::SetViewport { .. },
            )
            | UndoStep::Doc(_) => false,
        }
    }

    /// Identifies "same continuous gesture" for undo coalescing. The undo
    /// stack collapses consecutive steps with the same key into one entry
    /// (keeping the *first* "from" payload). Two viewport changes coalesce;
    /// two `MoveNodes` of the *same* grabbed node coalesce.
    pub fn gesture_key(&self) -> Option<GestureKey> {
        match self {
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
                | GraphStep::SetDisabled { .. }
                | GraphStep::DetachSubgraph { .. },
            )
            | UndoStep::Doc(
                DocStep::CloseTab { .. }
                | DocStep::RenameBoundaryPort { .. }
                | DocStep::RenameSubgraph { .. },
            ) => None,
        }
    }

    /// Fold two consecutive steps of the same gesture into one: keep
    /// `self`'s "from" half and adopt `next`'s "to" half. `None` for any
    /// pair that doesn't coalesce. The undo stack calls this after matching
    /// [`Self::gesture_key`] (so the pair is the same variant, and for
    /// `NodeDrag` the same node), but the match below re-checks the pairing
    /// so the fold stays self-contained — variant internals live here next
    /// to the step definitions, not in the stack. Keep this in sync with
    /// `gesture_key`.
    pub fn coalesce(&self, next: &UndoStep) -> Option<UndoStep> {
        match (self, next) {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKey {
    Viewport,
    NodeDrag(NodeId),
    TabSwitch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::data::StaticValue;
    use scenarium::prelude::FuncId;

    /// Add a bare `Func`-kind node to `doc`'s root graph + main view at
    /// `pos`, returning its id.
    fn add_node_at(doc: &mut Document, pos: Vec2) -> NodeId {
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let id = node.id;
        doc.graph.add(node);
        doc.main_view.view_nodes.add(ViewNode { id, pos });
        id
    }

    #[test]
    fn duplicate_intent_drops_or_keeps_external_by_flag() {
        // a -> b (internal edge, both selected); c -> b (external, c not
        // selected). b also has a Const on input 1. Selecting {a, b} must
        // duplicate a' and b', keep a'->b' and the Const, drop c->b.
        let mut doc = Document::default();
        let a = add_node_at(&mut doc, Vec2::new(0.0, 0.0));
        let b = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
        let c = add_node_at(&mut doc, Vec2::new(0.0, 100.0));
        doc.graph
            .set_input_binding(InputPort::new(b, 0), (a, 0).into());
        doc.graph.set_input_binding(
            InputPort::new(b, 1),
            Binding::Const(StaticValue::from(7i64)),
        );
        doc.graph
            .set_input_binding(InputPort::new(b, 2), (c, 0).into());
        doc.main_view.selected_nodes = [a, b].into_iter().collect();

        let Some(Intent::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
        }) = build_duplicate_intent(&doc, GraphRef::Main)
        else {
            panic!("expected a DuplicateNodes intent");
        };

        assert_eq!(nodes.len(), 2, "both selected nodes cloned");
        assert!(subscriptions.is_empty());
        // Fresh ids, offset positions.
        let new_ids: BTreeSet<NodeId> = nodes.iter().map(|(_, n)| n.id).collect();
        assert!(
            new_ids.is_disjoint(&doc.main_view.selected_nodes),
            "clones get fresh ids"
        );
        let a_clone = nodes
            .iter()
            .find(|(vn, _)| vn.pos == Vec2::new(0.0, 0.0) + DUPLICATE_OFFSET)
            .map(|(_, n)| n.id)
            .expect("a's clone offset from its origin");

        // Exactly two bindings survive: the internal a'->b' edge and the
        // Const; the external c->b edge (input 2) is gone.
        assert_eq!(bindings.len(), 2);
        let b_clone = nodes
            .iter()
            .find(|(vn, _)| vn.pos == Vec2::new(100.0, 0.0) + DUPLICATE_OFFSET)
            .map(|(_, n)| n.id)
            .unwrap();
        let internal = bindings
            .iter()
            .find(|(port, _)| port.port_idx == 0)
            .expect("a'->b' edge present");
        assert_eq!(internal.0.node_id, b_clone, "edge sinks into b's clone");
        match &internal.1 {
            Binding::Bind(src) => {
                assert_eq!(src.node_id, a_clone, "remapped to a's clone");
                assert_eq!(src.port_idx, 0);
            }
            other => panic!("expected Bind, got {other:?}"),
        }
        assert!(
            bindings
                .iter()
                .any(|(port, bind)| port.port_idx == 1 && matches!(bind, Binding::Const(_))),
            "const binding copied"
        );
        assert!(
            !bindings.iter().any(|(port, _)| port.port_idx == 2),
            "external edge dropped"
        );

        // With `include_incoming`, the same selection keeps the external
        // c -> b edge, the clone's input still pointing at the original c.
        // (Fresh build → fresh clone ids, so re-find b's clone by position.)
        let Some(Intent::DuplicateNodes {
            nodes: incoming_nodes,
            bindings: incoming,
            ..
        }) = build_duplicate_intent_for(&doc, GraphRef::Main, &doc.main_view.selected_nodes, true)
        else {
            panic!("expected a DuplicateNodes intent");
        };
        assert_eq!(incoming.len(), 3, "internal + const + kept external");
        let b_clone2 = incoming_nodes
            .iter()
            .find(|(vn, _)| vn.pos == Vec2::new(100.0, 0.0) + DUPLICATE_OFFSET)
            .map(|(_, n)| n.id)
            .unwrap();
        let external = incoming
            .iter()
            .find(|(port, _)| port.port_idx == 2)
            .expect("external edge kept");
        assert_eq!(external.0.node_id, b_clone2, "edge sinks into b's clone");
        match &external.1 {
            Binding::Bind(src) => {
                assert_eq!(src.node_id, c, "external source stays the original c");
                assert_eq!(src.port_idx, 0);
            }
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_intent_none_without_selection() {
        let mut doc = Document::default();
        add_node_at(&mut doc, Vec2::ZERO);
        assert!(build_duplicate_intent(&doc, GraphRef::Main).is_none());
    }
}
