//! The [`Intent`] / [`DocIntent`] / [`UndoStep`] / [`GraphStep`] /
//! [`DocStep`] / [`BatchScope`] / [`GestureKey`] type model, plus the
//! [`Refusal`] a commit answers with when no step comes out of it.
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
//! The same split runs the other way, by *scope*: an [`Intent`] always
//! edits one graph and is therefore always accompanied by a `GraphRef`,
//! while a [`DocIntent`] edits the document as a whole and names no graph
//! at all. Neither can be mistaken for the other, so no code path has to
//! carry a target it will not read, or ignore one it was handed.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{Binding, CacheMode, DataType, InputPort, Node, NodeId, Subscription};
use scenarium::{DetachedGraphInput, DetachedGraphOutput, DetachedNode, GraphDef, GraphId};
use serde::{Deserialize, Serialize};

use crate::core::document::dock::{DockLayout, DockOp, DockPath};
use crate::core::document::{BoundarySide, GraphRef, Viewport};

/// One scalar node property an editor can toggle — the payload of
/// [`Intent::SetNodeProperty`]. Both variants are geometry-neutral (changing
/// one never remeasures the node or reshapes a graph interface) and dirty
/// the document, so they share one intent / step rather than a variant each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NodeProperty {
    /// `Node::disabled` — excluded from execution unless explicitly seeded.
    Disabled(bool),
    /// `Node::cache` — where the node's output is cached (see [`CacheMode`]).
    RuntimeCache(CacheMode),
}

/// Why an intent never became an [`UndoStep`]. The split is by who is at
/// fault.
///
/// [`Quiet`](Self::Quiet) is the ordinary outcome of input that spans
/// frames: the item the intent named is gone, the change is already in
/// place, or the edit is refused by design. Callers drop it without a word.
///
/// [`Invalid`](Self::Invalid) means the payload could never have applied —
/// a nil or colliding identity, a non-finite position, a link to state the
/// document doesn't hold. It exists for the one caller whose payload is
/// untrusted: a script's, deserialized straight into an `Intent` by
/// `core::script::engine::decode_action`, so the reason travels back to it
/// instead of vanishing. No *widget* can build one — they read the
/// identities they emit out of the live document, so the worst they manage
/// is stale, which refuses [`Quiet`](Self::Quiet)ly — and the GUI commit
/// path asserts exactly that (`Editor::commit_widget_batch`, whose return
/// type has no room for a refusal in the first place).
#[derive(Debug)]
pub(crate) enum Refusal {
    Quiet,
    Invalid(String),
}

/// What the caller wants to change **in one graph**. Forward-only — no
/// `from` fields. Each variant says "set X to Y"; the consumer captures
/// the previous Y at commit time via
/// [`build_step`](crate::core::edit::intent::build::build_step).
///
/// Every variant here is meaningless without the graph it applies to, so
/// one always travels beside it — as a `GraphRef` argument on the commit
/// path, and as [`Queued::Scoped`](crate::core::edit::intent::sink::Queued)
/// in the frame's queue. A mutation with no graph to name is a
/// [`DocIntent`] instead.
///
/// **Adding a variant** — touch these spots:
///   1. add the variant here on `Intent`,
///   2. add the matching variant on [`GraphStep`] (edited through the
///      target's `EditScope`) or [`DocStep`] (writes state that sits
///      outside any one graph body — the dock layout, or a graph's
///      interface — so it resolves no scope), carrying
///      both the forward "to" and backward "from" payloads (or just
///      forward fields for pure-creation intents),
///   3. add an arm to
///      [`build_step`](crate::core::edit::intent::build::build_step) (read
///      `from` from `&Document` and combine with the intent's `to` into a
///      complete step) — and establish there *every* precondition the
///      variant's `apply` half assumes, since that arm is the only gate
///      between an untrusted payload and the document,
///   4. add an arm to the matching `apply_*` / `revert_*` fn in
///      [`crate::core::edit::intent::apply`],
///   5. add arms to `UndoStep::is_noop`,
///      `UndoStep::invalidates_cached_geometry`, and
///      `UndoStep::requires_reconcile` in
///      [`crate::core::edit::intent::query`] (all exhaustive — they won't
///      compile until you do),
///   6. update `UndoStep::gesture_key` (also in
///      [`crate::core::edit::intent::query`]) if the variant coalesces in
///      undo history.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Intent {
    /// Add one node that links state the document already resolves — a func,
    /// a built-in special, or a graph instance whose definition is in place.
    /// Bringing a definition along is [`Self::AddLocalGraph`]'s job.
    AddNode {
        /// Where the node lands on the canvas; its view item is created
        /// alongside it, at the top of the paint stack.
        pos: Vec2,
        node_id: NodeId,
        node: Node,
        /// Initial input bindings to seed alongside the node — the caller
        /// fills these with each input's func-declared default
        /// (`Binding::Const`) so a fresh node lands ready to run instead of
        /// fully unbound. Applied atomically with the node, so one undo
        /// removes node + seeds together.
        bindings: Vec<(InputPort, Binding)>,
    },
    /// Add a local definition *and* the first instance of it — the palette's
    /// row for a library graph, which localizes on instance.
    ///
    /// The two halves are one intent because they are one undo entry: a
    /// revert that left the definition behind would leave an orphan the
    /// palette still lists.
    AddLocalGraph {
        pos: Vec2,
        node_id: NodeId,
        graph_id: GraphId,
        def: Box<GraphDef>,
    },
    /// Instance a local definition the target graph already holds — the
    /// palette's row for one of its own nested graphs.
    AddLocalGraphInstance {
        pos: Vec2,
        node_id: NodeId,
        graph_id: GraphId,
    },
    /// Paste a set of pre-cloned nodes (fresh ids, offset positions) plus
    /// the connections *among* them, and select the copies. The caller
    /// (Ctrl+D duplicate) builds the clones + remapped wiring; `build_step`
    /// only captures the prior selection. One undo entry for the whole
    /// duplicate.
    DuplicateNodes {
        /// `(position, id, node)` per clone.
        nodes: Vec<(Vec2, NodeId, Node)>,
        bindings: Vec<(InputPort, Binding)>,
        subscriptions: Vec<Subscription>,
    },
    RemoveNode {
        node_id: NodeId,
    },
    /// Drag-move one or more selected node bodies in canvas-world
    /// coordinates. A
    /// multi-select drag moves the whole group as a single undo entry; a
    /// plain drag carries just the one grabbed item. `grabbed` is whichever
    /// member the pointer latched — it keys the drag gesture so consecutive
    /// frames coalesce.
    MoveSelection {
        grabbed: NodeId,
        /// `(item, target position)` per moved member, both kinds mixed.
        moves: Vec<(NodeId, Vec2)>,
    },
    RenameNode {
        node_id: NodeId,
        to: String,
    },
    SetInput {
        input: InputPort,
        to: Option<Binding>,
    },
    /// Replace the whole selection set. The rubber band, node/pin clicks,
    /// and Esc-deselect all funnel through this — the caller computes
    /// the desired final set and the undo layer captures the prior one.
    SetSelection {
        to: BTreeSet<NodeId>,
    },
    /// Lift an item — a node body or a pinned output's preview widget —
    /// to the top of its graph's paint stack: the end of `item_placements`,
    /// which is drawn last and so sits in front. Emitted when either kind
    /// is clicked or grabbed, so clicking brings it forward. The stack
    /// order lives in `item_placements`, so it persists across save/load and
    /// tab switches and walks with undo/redo — unlike the transient
    /// selection-recency stack it replaced.
    Raise {
        key: NodeId,
    },
    /// Set one scalar property of a node — its `disabled` flag or its cache
    /// [`CacheMode`] (see [`NodeProperty`]). Emitted by the header badges: a
    /// sink's `D` flips `Disabled` (ambient runs exclude it; an explicit node
    /// seed overrides it once); the `R`/`↓` chips each flip one bit of
    /// `RuntimeCache` (the disk bit persists the output so a reproducible node
    /// reloads instead of recomputing).
    SetNodeProperty {
        node_id: NodeId,
        to: NodeProperty,
    },
    /// Fork a private standalone copy of a `Graph(Local(_))` node's
    /// nested graph and re-point the node at it (the G-badge "Detach" action).
    /// The copy gets fresh ids and a cleared `origin`, so it diverges
    /// from any sibling instances *and* from the library it came from.
    /// `build_step` reads the source graph from the active graph; dropped
    /// when the node isn't a local graph instance.
    DetachGraph {
        node_id: NodeId,
    },
    SetViewport {
        to: Viewport,
    },
    /// Rename a subgraph definition's interface port. Scoped to the
    /// active `Local` target — `build_step` reads the `GraphId` from
    /// the drain `target`, so the intent only carries the side + index +
    /// new name. Dropped when the target isn't a graph interior.
    RenameBoundaryPort {
        side: BoundarySide,
        idx: usize,
        to: String,
    },
    /// Append a new interface port to the active graph interior's
    /// definition. Emitted right before the `SetInput` that wires the
    /// boundary placeholder it stands for — same batch, one undo entry.
    /// Scoped to the active `Local` target like
    /// [`Intent::RenameBoundaryPort`]; dropped elsewhere.
    AddBoundaryPort {
        side: BoundarySide,
        name: String,
        data_type: DataType,
    },
    /// Remove an interface port (the boundary port row's context menu).
    /// The step captures every binding and pin the removal severs — on
    /// both sides of the boundary — so undo restores the exact wiring.
    /// Dropped when the slot doesn't exist or its ports are pinned
    /// (unpin first; refusing beats reconciling preview widgets).
    RemoveBoundaryPort {
        side: BoundarySide,
        idx: usize,
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
}

/// What the caller wants to change **about the document as a whole** —
/// the mutations no single graph owns, and so the ones no `GraphRef`
/// describes.
///
/// Kept apart from [`Intent`] rather than sharing the enum with a target
/// nobody reads: these commit through
/// [`build_doc_step`](crate::core::edit::intent::build::build_doc_step),
/// which takes a `&Document` and nothing else, and the pipeline carries
/// them as [`Queued::Global`](crate::core::edit::intent::sink::Queued) with
/// no target beside them. The boundary-port intents are *not* here — they
/// produce [`DocStep`]s but read their `GraphId` off the graph they were
/// raised in, so they stay scoped.
///
/// Not serde: scripts drive graph edits only (`Intent`), so nothing
/// decodes a `DocIntent` from a payload.
///
/// **Adding a variant**: a matching [`DocStep`], an arm in `build_doc_step`
/// and in each `apply_doc` / `revert_doc`, plus the exhaustive `DocStep`
/// predicates in [`crate::core::edit::intent::query`].
#[derive(Debug)]
pub(crate) enum DocIntent {
    /// Mutate the dock layout (activate/close a tab, move one between
    /// panes, resize a split). `build_doc_step` snapshots the whole layout
    /// before/after (it's tiny), so every dock op is one uniform, trivially
    /// reversible step.
    Dock(DockOp),
    /// Rename a local graph's subgraph definition — it works regardless of
    /// which tab is active, since the `GraphId` names the definition
    /// outright. Drives the tab-strip's double-click-to-rename label.
    RenameGraph { id: GraphId, to: String },
}

/// What one undo entry's steps resolve against, recorded per batch by
/// [`ActionStack::push_current`](crate::core::edit::action_stack::ActionStack::push_current)
/// and read back on undo/redo — the step-level echo of the
/// [`Intent`] / [`DocIntent`] split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchScope {
    /// Built from [`Intent`]s against one graph: every [`GraphStep`] in the
    /// entry writes through that graph's `EditScope`. A [`DocStep`] sharing
    /// the entry (a boundary-port edit) carries its own `GraphId` and
    /// ignores this.
    Graph(GraphRef),
    /// Built from a [`DocIntent`]: the entry holds document-global steps
    /// only, and nothing in it resolves a graph.
    Document,
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
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum UndoStep {
    Graph(GraphStep),
    Doc(DocStep),
}

/// Steps applied through an `EditScope` (graph + view) for the batch's
/// `GraphRef` target.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum GraphStep {
    /// Pure creation: the "from" state is "node absent", which is
    /// implicit — undo removes the node by id and its new nested graph.
    ///
    /// The one undo representation behind all three add intents
    /// ([`Intent::AddNode`], [`Intent::AddLocalGraph`],
    /// [`Intent::AddLocalGraphInstance`]): they differ in what the *caller*
    /// may say and what `build_step` must resolve, not in what applying and
    /// reverting do. `graph` is `Some` only for the one that materializes a
    /// definition.
    AddNode {
        pos: Vec2,
        node_id: NodeId,
        node: Node,
        graph: Option<(GraphId, Box<GraphDef>)>,
        bindings: Vec<(InputPort, Binding)>,
    },
    /// Add a batch of nodes + their internal wiring and swap the
    /// selection to the copies. Undo removes every added node (which
    /// cascade-drops the added bindings/subscriptions) and restores
    /// `from_selection`. `nodes` carry fresh ids, so there's no prior
    /// state to capture beyond the selection.
    DuplicateNodes {
        nodes: Vec<(Vec2, NodeId, Node)>,
        bindings: Vec<(InputPort, Binding)>,
        subscriptions: Vec<Subscription>,
        from_selection: BTreeSet<NodeId>,
        to_selection: BTreeSet<NodeId>,
    },
    /// Pre-removal state lives entirely on the step: every reference
    /// into the doomed node, so undo can fully restore it.
    RemoveNode {
        detached: DetachedNode,
        /// The node's view item with the paint-stack slot it occupied —
        /// undo restores position *and* stacking exactly.
        item_placements: Vec<(usize, NodeId, Vec2)>,
        /// This node's selection membership — removal prunes it, undo re-adds.
        selected: Vec<NodeId>,
    },
    MoveSelection {
        grabbed: NodeId,
        /// `(item, from, to)` per moved member, both kinds mixed. An item
        /// missing at build time (node vanished or port unpinned
        /// mid-drag) is dropped, so this can be shorter than the intent's
        /// `moves`.
        moves: Vec<(NodeId, Vec2, Vec2)>,
    },
    RenameNode {
        node_id: NodeId,
        from: String,
        to: String,
    },
    SetInput {
        input: InputPort,
        from: Option<Binding>,
        to: Option<Binding>,
    },
    SetSelection {
        from: BTreeSet<NodeId>,
        to: BTreeSet<NodeId>,
    },
    /// Reorder within `item_placements` to raise an item (node body or pin
    /// preview) to the top of the paint stack. `from_index`/`to_index` are
    /// its slot before/after the raise, so apply slides it to `to_index`
    /// and revert slides it back — a stable reorder that leaves every
    /// other item's relative order intact.
    Raise {
        key: NodeId,
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
    /// Fork + re-point: a fresh graph joins the local map and the node
    /// swaps from `from_id` to `to_id`. Undo restores the old link.
    DetachGraph {
        node_id: NodeId,
        from_id: GraphId,
        to_id: GraphId,
        graph: Box<GraphDef>,
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
}

/// Document-global steps — they mutate fields that aren't scoped to a
/// single graph (active tab, the tab list, a graph's interface), so
/// they bypass the `EditScope` resolution entirely.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum DocStep {
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
    /// `graph_id` is resolved at build time so apply/revert are
    /// self-contained (don't need the drain target). Carries both names;
    /// the slot is addressed by `idx` directly (interface indices only
    /// move through recorded steps, so LIFO replay keeps them valid), with
    /// the expected current name as a stale-step guard.
    RenameBoundaryPort {
        graph_id: GraphId,
        side: BoundarySide,
        idx: usize,
        from: String,
        to: String,
    },
    /// Append (`apply`) / pop (`revert`) one interface port. `idx` is the
    /// list length at build time — the placeholder is always the trailing
    /// slot — so no remapping is involved on either half.
    AddBoundaryPort {
        graph_id: GraphId,
        side: BoundarySide,
        idx: usize,
        name: String,
        data_type: DataType,
    },
    /// Detach (`apply`) / attach (`revert`) one interface port with all
    /// the wiring it severs, via the scenarium detach/attach pair — the
    /// interface-port sibling of [`GraphStep::RemoveNode`].
    RemoveBoundaryPort {
        graph_id: GraphId,
        detached: DetachedBoundaryPort,
    },
    /// Rename a local graph. Self-contained on the step (both
    /// names) so apply/revert don't need to re-read the doc.
    RenameGraph {
        id: GraphId,
        from: String,
        to: String,
    },
}

/// The removed interface port a [`DocStep::RemoveBoundaryPort`] carries —
/// scenarium's detached record, tagged by side (the two sides detach
/// different spec types).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum DetachedBoundaryPort {
    Input(DetachedGraphInput),
    Output(DetachedGraphOutput),
}

/// Serde because [`DocStep::Dock`] stores its key on the step (the undo
/// stack packs steps with bitcode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GestureKey {
    Viewport,
    /// A group drag, keyed by whichever node the pointer latched, so two
    /// different grabbed nodes never coalesce.
    SelectionDrag(NodeId),
    TabSwitch,
    /// One divider's drag, keyed by the split's packed root path, so
    /// two different dividers never coalesce.
    DockResize(DockPath),
}
