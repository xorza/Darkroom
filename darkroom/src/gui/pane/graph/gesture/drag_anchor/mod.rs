//! The group drag: whichever selected node the pointer latched drags its
//! whole group alongside it, as one coalesced `GraphIntent::MoveSelection` per
//! frame.
//!
//! The caller owns the hit-testing that decides *what* got grabbed — a node
//! body or its title — and hands the result to [`GroupDrag::latch`].
//! Everything after that lives here in [`GroupDrag::advance`].

use glam::Vec2;
use palantir::{Ui, WidgetId};
use scenarium::NodeId;

use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use std::collections::BTreeSet;

use crate::gui::graph_ctx::GraphCtx;

/// One in-flight group drag, or none.
///
/// [`Self::advance`] is the whole per-frame lifecycle: the grabbed item's
/// node left the scene, a fresh gesture relatched on the same widget, the
/// drag released, or it moved and commits. Every committed position is
/// `start + drag_delta` against the snapshot taken at latch — not a running
/// integration over the moving widget — so a dropped frame can't accumulate
/// drift.
#[derive(Default, Debug)]
pub(crate) struct GroupDrag {
    anchor: Option<Anchor>,
}

/// The latched drag: what was grabbed, where every moving member started,
/// and whose response drives it.
#[derive(Debug)]
struct Anchor {
    /// The node the pointer grabbed. Names the drag in the emitted intent
    /// (so the edit layer knows which item the user is actually holding),
    /// and it is the node [`GroupDrag::advance`] checks against the scene.
    grabbed: NodeId,
    /// The graph pane the drag latched on. Several are on screen, and the
    /// gesture outlives the frame that started it, so the anchor travels
    /// with the anchor rather than being re-derived from whatever pane the
    /// pointer has since wandered over.
    /// Every node moving with this drag — and its position at drag start:
    /// the whole selection when the grabbed node was already selected,
    /// else just the grabbed one.
    start_positions: Vec<(NodeId, Vec2)>,
    /// The widget whose drag delta drives the gesture, captured at latch so
    /// later frames can `ui.response_for(widget_id)` without the caller
    /// having to remember which of its several grab targets started it.
    widget_id: WidgetId,
}

impl GroupDrag {
    /// Start (or replace) the gesture. `start_positions` includes the
    /// grabbed member itself.
    pub(crate) fn latch(
        &mut self,
        grabbed: NodeId,
        start_positions: Vec<(NodeId, Vec2)>,
        widget_id: WidgetId,
    ) {
        self.anchor = Some(Anchor {
            grabbed,
            start_positions,
            widget_id,
        });
    }

    /// Drop the gesture when the node owning the grabbed member has left the
    /// scene — a mid-drag delete (breaker swipe, undo). Left in place the
    /// anchor would emit a `MoveSelection` against a missing node, which
    /// panics in `build_step`, and could fire again if a fresh node reused
    /// the id.
    pub(crate) fn drop_if_owner_gone(&mut self, graph_ctx: GraphCtx<'_>) {
        let gone = self
            .anchor
            .as_ref()
            .is_some_and(|a| !graph_ctx.contains(a.grabbed));
        if gone {
            self.anchor = None;
        }
    }

    /// Advance one frame, pushing this frame's `GraphIntent::MoveSelection` when
    /// the drag is still held, and reporting whether it is. A caller that
    /// also latches fresh drags skips its own scan while this returns
    /// `true` — the gesture already owns the frame.
    ///
    /// Runs pre-record, so the move lands in `Document` before the pass that
    /// draws the moved items: they paint at the cursor in Pass A with no
    /// relayout retry.
    pub(crate) fn advance(&mut self, ui: &Ui, graph_ctx: GraphCtx<'_>, out: &mut Intents) -> bool {
        self.drop_if_owner_gone(graph_ctx);
        // Copy the ids out and drop the borrow, so the branches below can
        // clear the slot without cloning `start_positions` — only the
        // success path reads it, and that path never clears.
        let Some(widget_id) = self.anchor.as_ref().map(|a| a.widget_id) else {
            return false;
        };
        let resp = ui.response_for(widget_id);
        // `drag_started` on a still-active anchor means a *new* gesture just
        // latched on the same widget. Emitting with the stale start
        // positions would snap the group back to the previous gesture's
        // start point; the caller's latch scan picks the new one up instead.
        if resp.left.drag.started() {
            self.anchor = None;
            return false;
        }
        // No delta means the drag isn't latched anymore — release, or the
        // pointer left the surface.
        let Some(delta) = resp.left.drag.delta() else {
            self.anchor = None;
            return false;
        };
        // Palantir reports drag deltas in the widget's pre-transform frame,
        // which is the same canvas-world space item positions live in.
        let move_selection = self.anchor.as_ref().unwrap().resolve(delta);
        out.push(move_selection);
        true
    }
}

impl Anchor {
    /// This frame's `GraphIntent::MoveSelection`: every member's latch-time start
    /// plus Palantir's pre-transform drag `offset`.
    fn resolve(&self, offset: Vec2) -> GraphIntent {
        GraphIntent::MoveSelection {
            grabbed: self.grabbed,
            moves: self
                .start_positions
                .iter()
                .map(|(key, start)| (*key, *start + offset))
                .collect(),
        }
    }
}

/// Resolve the current selection into [`GroupDrag::latch`]'s
/// `start_positions` for a drag that grabbed an already-selected member —
/// shared by both callers, so the group moves the same way regardless of
/// which kind of member's press started it.
pub(crate) fn selected_group_positions(
    graph_ctx: GraphCtx<'_>,
    selected: &BTreeSet<NodeId>,
) -> Vec<(NodeId, Vec2)> {
    let holds = |key: NodeId| selected.contains(&key);
    let positions: Vec<(NodeId, Vec2)> = graph_ctx
        .nodes()
        .filter(|n| holds(n.id))
        .map(|n| (n.id, n.pos))
        .collect();
    positions
}

#[cfg(test)]
mod tests;
