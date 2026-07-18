//! The six exhaustive per-step predicates that drive the undo stack and the
//! per-frame pipeline: whether a step is a no-op, whether it needs a
//! relayout or a reconcile pass, whether it dirties the document, and its
//! undo-coalescing identity (`gesture_key` + `coalesce`).

use scenarium::Binding;

use crate::core::document::ItemRef;
use crate::core::edit::intent::types::{DocStep, GestureKey, GraphStep, UndoStep};

/// 1e-4 is the threshold below which two pan/scale samples are
/// considered the same gesture — keeps idle pan/zoom from polluting
/// the undo stack with sub-pixel deltas.
const VIEWPORT_EPS: f32 = 1e-4;

impl UndoStep {
    /// True when applying this step would leave the document unchanged.
    /// Filtered out post-`build_step` so phantom entries (re-selecting
    /// the same node, dragging zero pixels) don't pollute the undo stack.
    pub(crate) fn is_noop(&self) -> bool {
        match self {
            UndoStep::Graph(g) => g.is_noop(),
            UndoStep::Doc(d) => d.is_noop(),
        }
    }

    /// Whether replaying this step changes anything the layout engine
    /// reads (node positions, sizes, label text length, viewport
    /// transform). When true, `App::record` calls `ui.request_relayout()`
    /// after applying the batch so the next pass picks up the change.
    /// UI-only state with no measure/arrange input (selection, cache
    /// behavior, model-only bindings) returns false. Exhaustive on
    /// purpose — a new variant must declare its layout effect.
    pub(crate) fn requires_relayout(&self) -> bool {
        match self {
        // A dock change reshapes panes/strips (and can swap which graph
        // the scene renders); a port rename changes a label's width so
        // the node remeasures — all relayout the canvas.
        UndoStep::Doc(
            DocStep::Dock { .. }
            | DocStep::RenameBoundaryPort { .. }
            // Graph rename changes the tab-strip label's width.
            | DocStep::RenameGraph { .. },
        ) => true,
        UndoStep::Graph(g) => match g {
            GraphStep::AddNode { .. }
            | GraphStep::DuplicateNodes { .. }
            | GraphStep::RemoveNode { .. }
            | GraphStep::RenameNode { .. }
            // Forks an identical-interface def, so the node doesn't
            // resize — but it's a structural edit and rare, so eat one
            // relayout rather than reason about it staying in lockstep.
            | GraphStep::DetachGraph { .. } => true,
            // A pin-only drag repositions a decoration drawn past the
            // node's own rect — no remeasure, same as a viewport pan. A
            // node move (alone or as part of a mixed group drag) does need
            // one.
            GraphStep::MoveSelection { moves, .. } => moves
                .iter()
                .any(|(key, ..)| matches!(key, ItemRef::Node(_))),
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
            // Raising only reorders the paint stack — no node remeasures.
            | GraphStep::Raise { .. }
            // A node property (disable dims the body, a cache toggle flips a
            // badge fill) keeps the same rect — no remeasure.
            | GraphStep::SetNodeProperty { .. }
            // Event wiring paints a wire between existing glyphs — no
            // node remeasure.
            | GraphStep::SetSubscription { .. }
            // Flips a port's outline paint only — no remeasure.
            | GraphStep::SetOutputPinned { .. } => false,
        },
    }
    }

    /// Whether applying this step can change a graph's *derived interface*
    /// (`def.inputs`/`def.outputs`), so `reconcile_boundaries` must rerun
    /// before the next scene rebuild. Only interior boundary wiring and
    /// instance bindings feed that derivation, so any edit that touches a
    /// binding or the node set qualifies; pure view/selection/cache/tab edits
    /// (and boundary-port *renames*, which reconcile preserves) never do.
    /// Conservative on `SetInput` — a const-value edit on a plain func port
    /// can't change an interface, but filtering that needs a doc lookup, and
    /// reconcile is an idempotent no-op there anyway. Exhaustive on purpose.
    pub(crate) fn requires_reconcile(&self) -> bool {
        match self {
            UndoStep::Graph(
                GraphStep::AddNode { .. }
                | GraphStep::RemoveNode { .. }
                | GraphStep::DuplicateNodes { .. }
                | GraphStep::SetInput { .. }
                | GraphStep::DetachGraph { .. },
            ) => true,
            UndoStep::Graph(
                GraphStep::MoveSelection { .. }
                | GraphStep::RenameNode { .. }
                | GraphStep::SetSelection { .. }
                | GraphStep::Raise { .. }
                | GraphStep::SetNodeProperty { .. }
                | GraphStep::SetViewport { .. }
                // Subscriptions don't feed a graph's derived interface.
                | GraphStep::SetSubscription { .. }
                // Nor does pinning an output — it's not wiring at all.
                | GraphStep::SetOutputPinned { .. },
            )
            | UndoStep::Doc(_) => false,
        }
    }

    /// Whether applying or reverting this step changes *saved* document
    /// content — graph data or node layout — as opposed to pure
    /// navigation (camera, selection, tab focus), which isn't worth
    /// prompting to save on exit. Drives `Editor::dirty`. Exhaustive so a
    /// new step variant must declare which side it's on rather than
    /// silently defaulting.
    pub(crate) fn dirties_document(&self) -> bool {
        match self {
            // A structural dock op (a tab moved or split into its own
            // pane) is invested arrangement work worth the exit prompt;
            // activations, closes, and ratio nudges stay navigation.
            UndoStep::Doc(DocStep::Dock { structural, .. }) => *structural,
            // Navigation only — panning, zooming, selecting, or
            // restacking is view state the user doesn't "save".
            // Stacking order rides in `item_placements` and still writes on any
            // save (like selection), but a bare restack shouldn't nag on exit.
            UndoStep::Graph(
                GraphStep::SetSelection { .. }
                | GraphStep::Raise { .. }
                | GraphStep::SetViewport { .. },
            ) => false,
            // Graph data + node layout — real edits worth persisting.
            UndoStep::Graph(
                GraphStep::AddNode { .. }
                | GraphStep::DuplicateNodes { .. }
                | GraphStep::RemoveNode { .. }
                | GraphStep::MoveSelection { .. }
                | GraphStep::RenameNode { .. }
                | GraphStep::SetInput { .. }
                | GraphStep::SetNodeProperty { .. }
                | GraphStep::DetachGraph { .. }
                | GraphStep::SetSubscription { .. }
                | GraphStep::SetOutputPinned { .. },
            )
            | UndoStep::Doc(DocStep::RenameBoundaryPort { .. } | DocStep::RenameGraph { .. }) => {
                true
            }
        }
    }

    /// Identifies "same continuous gesture" for undo coalescing. The undo
    /// stack collapses consecutive steps with the same key into one entry
    /// (keeping the *first* "from" payload). Two viewport changes coalesce;
    /// two `MoveSelection`s of the *same* grabbed item coalesce.
    pub(crate) fn gesture_key(&self) -> Option<GestureKey> {
        match self {
            UndoStep::Graph(GraphStep::SetViewport { .. }) => Some(GestureKey::Viewport),
            UndoStep::Graph(GraphStep::MoveSelection { grabbed, .. }) => {
                Some(GestureKey::SelectionDrag(*grabbed))
            }
            // The key was derived from the dock intent at build time:
            // tab-switch bursts and one divider's drag frames collapse
            // into single entries; a close or move never coalesces.
            UndoStep::Doc(DocStep::Dock { key, .. }) => *key,
            // Everything else is its own undo entry.
            UndoStep::Graph(
                GraphStep::AddNode { .. }
                | GraphStep::DuplicateNodes { .. }
                | GraphStep::RemoveNode { .. }
                | GraphStep::RenameNode { .. }
                | GraphStep::SetInput { .. }
                | GraphStep::SetSelection { .. }
                | GraphStep::Raise { .. }
                | GraphStep::SetNodeProperty { .. }
                | GraphStep::DetachGraph { .. }
                | GraphStep::SetSubscription { .. }
                | GraphStep::SetOutputPinned { .. },
            )
            | UndoStep::Doc(DocStep::RenameBoundaryPort { .. } | DocStep::RenameGraph { .. }) => {
                None
            }
        }
    }

    /// Fold two consecutive steps of the same gesture into one: keep
    /// `self`'s "from" half and adopt `next`'s "to" half. `None` for any
    /// pair that doesn't coalesce. The undo stack calls this after matching
    /// [`Self::gesture_key`] (so the pair is the same variant, and for
    /// `SelectionDrag` the same grabbed member), but the match below
    /// re-checks the pairing
    /// so the fold stays self-contained — variant internals live here next
    /// to the step definitions, not in the stack. Keep this in sync with
    /// `gesture_key`.
    pub(crate) fn coalesce(&self, next: &UndoStep) -> Option<UndoStep> {
        match (self, next) {
            (
                UndoStep::Graph(GraphStep::SetViewport { from, .. }),
                UndoStep::Graph(GraphStep::SetViewport { to, .. }),
            ) => Some(UndoStep::Graph(GraphStep::SetViewport {
                from: *from,
                to: *to,
            })),
            (
                UndoStep::Graph(GraphStep::MoveSelection {
                    grabbed,
                    moves: prev_moves,
                }),
                UndoStep::Graph(GraphStep::MoveSelection {
                    moves: next_moves, ..
                }),
            ) => {
                // Same gesture (matched `SelectionDrag` key) ⇒ same group;
                // keep each member's original `from`, adopt its latest `to`.
                let moves = prev_moves
                    .iter()
                    .map(|(key, from, prev_to)| {
                        let to = next_moves
                            .iter()
                            .find(|(k, _, _)| k == key)
                            .map(|(_, _, t)| *t)
                            .unwrap_or(*prev_to);
                        (*key, *from, to)
                    })
                    .collect();
                Some(UndoStep::Graph(GraphStep::MoveSelection {
                    grabbed: *grabbed,
                    moves,
                }))
            }
            (
                UndoStep::Doc(DocStep::Dock {
                    from,
                    key,
                    structural,
                    ..
                }),
                UndoStep::Doc(DocStep::Dock { to, .. }),
            ) => Some(UndoStep::Doc(DocStep::Dock {
                from: from.clone(),
                to: to.clone(),
                key: *key,
                // Only non-structural ops carry a gesture key, so a
                // coalesced run can't smuggle a move past the flag.
                structural: *structural,
            })),
            _ => None,
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
            | GraphStep::DetachGraph { .. } => false,
            GraphStep::DuplicateNodes { nodes, .. } => nodes.is_empty(),
            GraphStep::MoveSelection { moves, .. } => moves.iter().all(|(_, from, to)| from == to),
            GraphStep::RenameNode { from, to, .. } => from == to,
            GraphStep::SetInput { from, to, .. } => from == to,
            GraphStep::SetSelection { from, to } => from == to,
            // Already on top (its slot is the last one) → nothing to raise.
            GraphStep::Raise {
                from_index,
                to_index,
                ..
            } => from_index == to_index,
            GraphStep::SetNodeProperty { from, to, .. } => from == to,
            GraphStep::SetOutputPinned { from, to, .. } => from == to,
            GraphStep::SetViewport { from, to } => {
                (from.pan - to.pan).length_squared() < VIEWPORT_EPS * VIEWPORT_EPS
                    && (from.zoom - to.zoom).abs() < VIEWPORT_EPS
            }
            GraphStep::SetSubscription { from, to, .. } => from == to,
        }
    }
}

impl DocStep {
    fn is_noop(&self) -> bool {
        match self {
            // Covers every degenerate dock op in one comparison: same-tab
            // activation, a refused close/move, an unchanged ratio.
            DocStep::Dock { from, to, .. } => from == to,
            DocStep::RenameBoundaryPort { from, to, .. } => from == to,
            DocStep::RenameGraph { from, to, .. } => from == to,
        }
    }
}
