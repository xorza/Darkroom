//! The [`GraphIntent`] / [`UndoStep`] / [`UndoStep`] / [`GestureKey`] type
//! model, plus the
//! [`Refusal`] a commit answers with when no step comes out of it.
//!
//! An [`GraphIntent`] is "what the caller wants the graph to look like
//! after"; it carries no history. To make the change reversible, we
//! pair the intent with a snapshot of the slot it overwrites. Rather
//! than carrying that snapshot in a sibling enum, [`UndoStep`] folds
//! both halves into one variant per kind: every variant has both the
//! "from" payload (for revert) and the "to" payload (for forward
//! apply). Type-level enforcement means an `UndoStep` can never be
//! constructed inconsistently — there's no `(GraphIntent::A, Snapshot::B)`
//! mismatch to worry about at runtime.
//!
//! The same split runs the other way, by *scope*: a [`GraphIntent`] edits the
//! graph, while a [`DockOp`](crate::core::document::dock::DockOp) edits the
//! layout around it. Neither can be mistaken for the other, so no code path has
//! to carry state it will not read.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::DetachedNode;
use scenarium::{Binding, CacheMode, InputPort, Node, NodeId, Subscription};
use serde::{Deserialize, Serialize};

use crate::core::document::Viewport;

/// One scalar node property an editor can toggle — the payload of
/// [`GraphIntent::SetNodeProperty`]. Both variants are geometry-neutral (changing
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
/// document doesn't hold. Nothing that raises an intent can legitimately
/// build one: widgets read the identities they emit out of the live
/// document, so the worst they manage is stale, which refuses
/// [`Quiet`](Self::Quiet)ly. An `Invalid` is therefore our own bug, and
/// `Editor::commit_widget_batch` panics on it rather than swallowing it.
/// It stays a value rather than a panic *here* so the check reads as a
/// precondition on the way in, and so a test can assert which one broke.
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
/// Every variant here edits the graph, and travels the frame's queue as its
/// `Graph` tier. A mutation of the layout instead of the graph is a
/// [`DockOp`](crate::core::document::dock::DockOp), which travels the same
/// queue in the `View` tier.
///
/// **Adding a variant** — touch these spots:
///   1. add the variant here on `GraphIntent`,
///   2. add the matching variant on [`UndoStep`], edited through the
///      target's `EditScope`, carrying both the forward "to" and backward
///      "from" payloads (or just forward fields for pure-creation intents),
///   3. add an arm to
///      [`build_step`](crate::core::edit::intent::build::build_step) (read
///      `from` from `&Document` and combine with the intent's `to` into a
///      complete step) — and establish there *every* precondition the
///      variant's `apply` half assumes, since that arm is the only gate
///      between an untrusted payload and the document,
///   4. add an arm to the matching `apply_*` / `revert_*` fn in
///      [`crate::core::edit::intent::apply`],
///   5. add arms to `UndoStep::is_noop` and
///      `UndoStep::invalidates_cached_geometry` in
///      [`crate::core::edit::intent::query`] (both exhaustive — they won't
///      compile until you do),
///   6. update `UndoStep::gesture_key` (also in
///      [`crate::core::edit::intent::query`]) if the variant coalesces in
///      undo history.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum GraphIntent {
    /// Add one node that links state the document already resolves: a func in
    /// the library, or a built-in special.
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
    SetViewport {
        to: Viewport,
    },
    /// Add (`subscribe = true`) or remove (`false`) an event subscription:
    /// `subscriber` ← `emitter`'s event `event_idx`. An event wire dropped on,
    /// or severed from, a subscription pin. Idempotent — a no-op when the
    /// subscription already matches. Lowers to the single reversible
    /// [`UndoStep::SetSubscription`], subscribe and unsubscribe being exact
    /// inverses.
    SetSubscription {
        emitter: NodeId,
        event_idx: usize,
        subscriber: NodeId,
        subscribe: bool,
    },
}

/// Self-contained undo-stack entry. Each leaf variant carries both
/// halves: the forward "to" payload (read by `apply_step`) and the
/// backward "from" payload (read by `revert_step`). Built from an
/// [`GraphIntent`] via `build_step`, which captures the pre-mutation state
/// from `&Document` at commit time.
///
/// Steps applied through an `EditScope` — the document's graph and the view
/// metadata beside it.
///
/// Only graph edits are undoable: pane arrangement applies straight to the
/// layout and records nothing, so Ctrl+Z walks past a tab switch to the last
/// edit that changed the graph. That is why this is the whole step
/// vocabulary rather than one arm of a wider one.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum UndoStep {
    /// Pure creation: the "from" state is "node absent", which is
    /// implicit — undo removes the node by id.
    AddNode {
        pos: Vec2,
        node_id: NodeId,
        node: Node,
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
    /// write the [`NodeProperty`] into its field. See [`GraphIntent::SetNodeProperty`].
    SetNodeProperty {
        node_id: NodeId,
        from: NodeProperty,
        to: NodeProperty,
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

/// Serde because a step stores its key (the undo stack packs steps with
/// bitcode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum GestureKey {
    Viewport,
    /// A group drag, keyed by whichever node the pointer latched, so two
    /// different grabbed nodes never coalesce.
    SelectionDrag(NodeId),
}
