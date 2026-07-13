//! The [`Intent`] / [`UndoStep`] / [`GraphStep`] / [`DocStep`] /
//! [`GestureKey`] type model.
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

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::graph::subgraph::{SubgraphDef, SubgraphId};
use scenarium::graph::{Binding, CacheMode, InputPort, Node, NodeId, OutputPort, Subscription};
use serde::{Deserialize, Serialize};

use crate::core::document::dock::{DockLayout, DockOp, DockPath};
use crate::core::document::view_node::ViewNode;
use crate::core::document::{BoundarySide, SelectionKey, Viewport};

/// One scalar node property an editor can toggle — the payload of
/// [`Intent::SetNodeProperty`]. Both variants are geometry-neutral (changing
/// one never remeasures the node or reshapes a subgraph interface) and dirty
/// the document, so they share one intent / step rather than a variant each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeProperty {
    /// `Node::disabled` — excluded from execution, skipped at flatten time.
    Disabled(bool),
    /// `Node::cache` — where the node's output is cached (see [`CacheMode`]).
    RuntimeCache(CacheMode),
}

/// What the caller wants to change. Forward-only — no `from` fields.
/// Each variant says "set X to Y"; the consumer captures the previous
/// Y at commit time via
/// [`build_step`](crate::core::edit::intent::build::build_step).
///
/// **Adding a variant** — touch these spots:
///   1. add the variant here on `Intent`,
///   2. add the matching variant on [`GraphStep`] (graph-scoped, edited
///      through an `EditScope`) or [`DocStep`] (document-global), carrying
///      both the forward "to" and backward "from" payloads (or just
///      forward fields for pure-creation intents),
///   3. add an arm to
///      [`build_step`](crate::core::edit::intent::build::build_step) (read
///      `from` from `&Document` and combine with the intent's `to` into a
///      complete step),
///   4. add an arm to the matching `apply_*` / `revert_*` fn in
///      [`crate::core::edit::intent::apply`],
///   5. add arms to `UndoStep::is_noop` and `UndoStep::requires_relayout` in
///      [`crate::core::edit::intent::query`] (both exhaustive — they won't
///      compile until you do),
///   6. update `UndoStep::gesture_key` (also in
///      [`crate::core::edit::intent::query`]) if the variant coalesces in
///      undo history.
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
    /// Drag-move one or more selected items — node bodies and/or
    /// pinned-output preview widgets — in canvas-world coordinates. A
    /// multi-select drag moves the whole group as a single undo entry; a
    /// plain drag carries just the one grabbed item. `grabbed` is whichever
    /// member the pointer latched — it keys the drag gesture so consecutive
    /// frames coalesce. Either list may be empty (a lone node drag has no
    /// pins; a lone pin drag/creation has no nodes).
    MoveSelection {
        grabbed: SelectionKey,
        nodes: Vec<(NodeId, Vec2)>,
        pins: Vec<(OutputPort, Vec2)>,
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
    /// Replace the whole selection set. The rubber band, node/pin clicks,
    /// and Esc-deselect all funnel through this — the caller computes
    /// the desired final set and the undo layer captures the prior one.
    SetSelection {
        to: BTreeSet<SelectionKey>,
    },
    /// Lift `node_id` to the top of its graph's paint stack — the end of
    /// `view_nodes`, which is drawn last and so sits in front. Emitted
    /// when a node is clicked or grabbed, so clicking a node brings it
    /// forward. The stack order lives in `view_nodes`, so it persists
    /// across save/load and tab switches and walks with undo/redo — unlike
    /// the transient selection-recency stack it replaced.
    RaiseNode {
        node_id: NodeId,
    },
    /// Set one scalar property of a node — its `disabled` flag or its cache
    /// [`CacheMode`] (see [`NodeProperty`]). Emitted by the header badges: `D`
    /// flips `Disabled` (skips the node at flatten time); the `R`/`↓` chips each
    /// flip one bit of `RuntimeCache` (the disk bit persists the output so a
    /// reproducible node reloads instead of recomputing).
    SetNodeProperty {
        node_id: NodeId,
        to: NodeProperty,
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
        to: Viewport,
    },
    /// Mutate the dock layout (activate/close a tab, move one between
    /// panes, resize a split). Document-global; `build_step` snapshots
    /// the whole layout before/after (it's tiny), so every dock op is
    /// one uniform, trivially reversible step.
    Dock(DockOp),
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
    /// Add (`subscribe = true`) or remove (`false`) an event subscription:
    /// `subscriber` ← `emitter`'s event `event_idx`. An event wire dropped on,
    /// or severed from, a subscription pin. Idempotent — a no-op when the
    /// subscription already matches. Lowers to the single reversible
    /// [`GraphStep::SetSubscription`], subscribe and unsubscribe being exact
    /// inverses.
    SetSubscription {
        emitter: NodeId,
        event_idx: usize,
        subscriber: NodeId,
        subscribe: bool,
    },
    /// Pin (`pinned = true`) or unpin (`false`) an output port, keeping it
    /// computed and read even with no in-graph consumer (e.g. a GUI
    /// inspector reading it live). Idempotent — a no-op when the flag
    /// already matches. Cmd(/Ctrl)+click on an output port circle, or its
    /// context-menu toggle — but also reachable unchecked from a script's
    /// generic `apply()` (see `core::script::register_mutations`), so a
    /// stale or bogus `node_id` must drop quietly, not crash.
    SetOutputPinned {
        node_id: NodeId,
        port_idx: usize,
        pinned: bool,
    },
}

/// Self-contained undo-stack entry. Each leaf variant carries both
/// halves: the forward "to" payload (read by `apply_step`) and the
/// backward "from" payload (read by `revert_step`). Built from an
/// [`Intent`] via `build_step`, which captures the pre-mutation state
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

/// Steps applied through an `EditScope` (graph + view) for the batch's
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
        from_selection: BTreeSet<SelectionKey>,
        to_selection: BTreeSet<SelectionKey>,
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
        /// Every pinned output's custom satellite position on this node
        /// (`EditScope::remove_node` prunes them along with everything
        /// else), captured so undo can fully restore it.
        pin_positions: Vec<(OutputPort, Vec2)>,
        was_selected: bool,
    },
    MoveSelection {
        grabbed: SelectionKey,
        /// `(node_id, from, to)` per moved node. A node missing at build
        /// time is dropped, so this can be shorter than the intent's `nodes`.
        node_moves: Vec<(NodeId, Vec2, Vec2)>,
        /// `(port, from, to)` per moved pin. Every pinned port is seeded
        /// with an explicit position the moment it's pinned (see
        /// `crate::gui::canvas::pin_ui::PinUi::apply`), so `from` is always
        /// its current stored position — a port whose node vanished
        /// mid-drag is dropped, like a missing node above.
        pin_moves: Vec<(OutputPort, Vec2, Vec2)>,
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
        from: BTreeSet<SelectionKey>,
        to: BTreeSet<SelectionKey>,
    },
    /// Reorder within `view_nodes` to raise a node to the top of the paint
    /// stack. `from_index`/`to_index` are its slot before/after the raise,
    /// so apply slides it to `to_index` and revert slides it back — a
    /// stable reorder that leaves every other node's relative order intact.
    RaiseNode {
        node_id: NodeId,
        from_index: usize,
        to_index: usize,
    },
    /// Set a scalar node property (disable flag or cache mode). One step backs
    /// both, since they're geometry-neutral and apply/revert identically —
    /// write the [`NodeProperty`] into its field. See [`Intent::SetNodeProperty`].
    SetNodeProperty {
        node_id: NodeId,
        from: NodeProperty,
        to: NodeProperty,
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
        from: Viewport,
        to: Viewport,
    },
    /// Add or remove an event subscription. `from`/`to` are the
    /// subscribed-state booleans, so apply/revert just (un)subscribe to
    /// match — subscribe and unsubscribe are exact inverses, so one step
    /// type backs both the `Subscribe` and `Unsubscribe` intents.
    SetSubscription {
        emitter: NodeId,
        event_idx: usize,
        subscriber: NodeId,
        from: bool,
        to: bool,
    },
    /// Mark or clear whether an output port is pinned. `from`/`to` are the
    /// pinned-state booleans — exact inverses, one step type backs the toggle.
    /// `was_selected` is whether the pin's preview widget was selected
    /// *before* this edit — unpinning drops its selection membership (the
    /// widget disappears with it), so a revert back to `pinned` restores it,
    /// mirroring `RemoveNode`'s `was_selected`.
    SetOutputPinned {
        node_id: NodeId,
        port_idx: usize,
        from: bool,
        to: bool,
        was_selected: bool,
    },
}

/// Document-global steps — they mutate fields that aren't scoped to a
/// single graph (active tab, the tab list, a subgraph's interface), so
/// they bypass the `EditScope` resolution entirely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocStep {
    /// Whole-layout snapshot around one dock op (activate/close/move/
    /// resize) — the tree is a handful of nodes, so both halves ride the
    /// step and apply/revert are plain assignments. `key` is the gesture
    /// this op coalesces under (a switch burst, one divider's drag);
    /// `structural` marks a [`DockOp::MoveTab`] (a split or a move —
    /// invested arrangement work, so it dirties the document, unlike
    /// activations/closes/ratio nudges). Both derived from the op at
    /// build time.
    Dock {
        from: DockLayout,
        to: DockLayout,
        key: Option<GestureKey>,
        structural: bool,
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

/// Serde because [`DocStep::Dock`] stores its key on the step (the undo
/// stack packs steps with bitcode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GestureKey {
    Viewport,
    /// A group drag, keyed by whichever item the pointer latched — a node
    /// body or a pin preview widget — so two different grabbed items never
    /// coalesce.
    SelectionDrag(SelectionKey),
    TabSwitch,
    /// One divider's drag, keyed by the split's packed root path, so
    /// two different dividers never coalesce.
    DockResize(DockPath),
}
