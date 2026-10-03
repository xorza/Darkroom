//! The group drag: whichever selected node the pointer latched drags its
//! whole group alongside it, one `GraphIntent::MoveSelection` per frame, all
//! folding into one undo entry.
//!
//! The caller owns the hit-testing that decides *what* got grabbed — a node
//! body or its title — and hands the result to [`GroupDrag::latch`].
//! Everything after that lives here in [`GroupDrag::advance`].

use std::sync::Arc;

use glam::Vec2;
use palantir::{Ui, WidgetId};
use scenarium::NodeId;

use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::requests::Requests;

/// One in-flight group drag, or none.
///
/// [`Self::advance`] is the whole per-frame lifecycle: the grabbed item's
/// node left the scene, a fresh gesture relatched on the same widget, Esc
/// cancelled it, the drag released, or it moved and commits. Every committed
/// position is `start + drag_delta` against the snapshot taken at latch — not
/// a running integration over the moving widget — so a dropped frame can't
/// accumulate drift.
#[derive(Default, Debug)]
pub(crate) struct GroupDrag {
    anchor: Option<Anchor>,
}

/// The latched drag: what was grabbed, where every moving member started,
/// and whose response drives it.
#[derive(Debug)]
struct Anchor {
    /// The node the pointer grabbed, which [`GroupDrag::advance`] checks
    /// against the scene.
    grabbed: NodeId,
    /// Every node moving with this drag, at its position when the drag
    /// latched: the whole selection when the grabbed node was already
    /// selected, else just the grabbed one. Shared with each frame's intent,
    /// so a frame copies no list.
    members: Arc<[DragStart]>,
    /// The widget whose drag delta drives the gesture, captured at latch so
    /// later frames can `ui.response_for(widget_id)` without the caller
    /// having to remember which of its several grab targets started it.
    widget_id: WidgetId,
    /// The undo gesture every frame of this drag folds into.
    gesture: GestureId,
}

impl GroupDrag {
    /// Drop any latched drag. Called on the tab-switch reset — a drag left
    /// latched while the canvas was away would otherwise resume when it
    /// comes back.
    pub(crate) fn reset(&mut self) {
        self.anchor = None;
    }

    /// Whether a drag is latched.
    pub(crate) fn in_flight(&self) -> bool {
        self.anchor.is_some()
    }

    /// Start (or replace) the gesture. `members` includes the grabbed node
    /// itself.
    pub(crate) fn latch(
        &mut self,
        grabbed: NodeId,
        members: Arc<[DragStart]>,
        widget_id: WidgetId,
        out: &mut Requests,
    ) {
        self.anchor = Some(Anchor {
            grabbed,
            members,
            widget_id,
            gesture: out.open_gesture(),
        });
    }

    /// Advance one frame, pushing this frame's `GraphIntent::MoveSelection`
    /// while the drag is held, and reporting whether the drag owned the frame.
    /// A caller that also latches fresh drags skips its own scan while this
    /// returns `true`.
    ///
    /// `cancelled` (Esc) puts every member back where the drag latched it and
    /// ends the drag; the undo gesture then ends where it began and records
    /// nothing.
    ///
    /// Runs pre-record, so the move lands in `Document` before the pass that
    /// draws the moved items: they paint at the cursor in Pass A with no
    /// relayout retry.
    pub(crate) fn advance(
        &mut self,
        ui: &Ui,
        graph_ctx: GraphCtx<'_>,
        cancelled: bool,
        out: &mut Requests,
    ) -> bool {
        // A node deleted mid-drag (breaker swipe, undo) takes its drag with
        // it: left in place the anchor would emit against a missing node, and
        // could fire again if a fresh node reused the id.
        if self
            .anchor
            .as_ref()
            .is_some_and(|anchor| !graph_ctx.contains(anchor.grabbed))
        {
            self.anchor = None;
        }
        let Some(anchor) = &self.anchor else {
            return false;
        };
        let resp = ui.response_for(anchor.widget_id);
        // `drag_started` on a still-active anchor means a *new* gesture just
        // latched on the same widget. Emitting with the stale start
        // positions would snap the group back to the previous gesture's
        // start point; the caller's latch scan picks the new one up instead.
        if resp.left.drag.started() {
            self.anchor = None;
            return false;
        }
        if cancelled {
            out.push_graph(anchor.frame(Vec2::ZERO));
            self.anchor = None;
            return true;
        }
        // No delta means the drag isn't latched anymore — release, or the
        // pointer left the surface.
        let Some(delta) = resp.left.drag.delta() else {
            self.anchor = None;
            return false;
        };
        // Palantir reports drag deltas in the widget's pre-transform frame,
        // which is the same canvas-world space item positions live in.
        out.push_graph(anchor.frame(delta));
        true
    }
}

impl Anchor {
    /// The frame of this drag that puts every member at its start plus
    /// `offset`.
    fn frame(&self, offset: Vec2) -> GraphIntent {
        GraphIntent::MoveSelection {
            gesture: self.gesture,
            members: Arc::clone(&self.members),
            offset,
        }
    }
}

/// The selection's members at their current positions — [`GroupDrag::latch`]'s
/// `members` for a drag that grabbed an already-selected member.
pub(crate) fn selected_group(dcx: DrawCtx<'_>) -> Arc<[DragStart]> {
    dcx.graph_ctx()
        .nodes()
        .filter(|n| dcx.is_selected(n.id))
        .map(|n| DragStart {
            node: n.id,
            pos: n.pos,
        })
        .collect()
}

#[cfg(test)]
mod tests;
