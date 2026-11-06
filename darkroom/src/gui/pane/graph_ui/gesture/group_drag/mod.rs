//! The group drag: whichever selected node or tie the pointer latched drags
//! its whole group alongside it, one `GraphIntent::MoveSelection` per frame,
//! all folding into one undo entry.
//!
//! The caller owns the hit-testing that decides *what* got grabbed — a node
//! body or its title, or a tie's band — and hands the result to
//! [`GroupDrag::latch`].
//! Everything after that lives here in [`GroupDrag::input`].

use std::collections::BTreeSet;
use std::rc::Rc;

use glam::Vec2;
use palantir::{Input, WidgetId};
use scenarium::NodeId;

use crate::core::document::graph_view::GraphView;
use crate::core::document::item_id::ItemId;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::gui::frame::input_stage::InputStage;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::unsettled_ctx::UnsettledCtx;

/// One in-flight group drag, or none.
///
/// [`Self::input`] is the whole per-frame lifecycle: the grabbed item left
/// the view, a fresh gesture relatched on the same widget, Esc
/// cancelled it, the drag released, or it moved and commits. Every committed
/// position is `start + drag_delta` against the snapshot taken at latch — not
/// a running integration over the moving widget — so a dropped frame can't
/// accumulate drift.
#[derive(Default, Debug)]
pub(crate) struct GroupDrag {
    anchor: Option<Anchor>,
}

/// What a press grabbed: the node or tie, every item moving with it, and the
/// widget whose drag drives them.
#[derive(Debug)]
pub(crate) struct Grab {
    /// The item the pointer grabbed, which [`GroupDrag::input`] checks
    /// against the view.
    pub(crate) item: ItemId,
    /// Every item moving with this drag, at its position when the drag
    /// latched: the whole selection when the grabbed node was already
    /// selected, else just the grabbed one, with the ties inside the moving
    /// nodes ([`with_inner_ties`]). Includes the grabbed node itself.
    /// Shared with each frame's intent, so a frame copies no list.
    pub(crate) members: Rc<[DragStart]>,
    /// The widget whose drag delta drives the gesture, captured at latch so
    /// later frames can `ui.response_for(handle)` without the caller having
    /// to remember which of its several grab targets started it.
    pub(crate) handle: WidgetId,
}

/// What a press latches: what it grabbed, and the edits that land with it, in the drag's one undo
/// gesture, so Esc and a Ctrl+Z take them back with the move.
#[derive(Debug)]
pub(crate) struct GrabStart {
    pub(crate) grab: Grab,
    /// A selection the press made, or the node a preview drag spawns.
    pub(crate) joined: Vec<GraphIntent>,
}

/// The latched drag: what was grabbed, and the undo gesture every frame of
/// it folds into.
#[derive(Debug)]
struct Anchor {
    grab: Grab,
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
    pub(crate) const fn in_flight(&self) -> bool {
        self.anchor.is_some()
    }

    /// Start (or replace) the gesture on what `start` grabbed, and move it to where the pointer
    /// has travelled, as a latch is past the drag threshold. A frame with no later input still
    /// shows the move.
    pub(crate) fn latch(&mut self, start: GrabStart, input: &Input, out: &mut Requests) {
        let GrabStart { grab, joined } = start;
        let gesture = out.open_gesture();
        out.extend_graph_in(gesture, joined);
        let anchor = Anchor { grab, gesture };
        if let Some(delta) = input.response_for(anchor.grab.handle).left.drag.delta() {
            out.push_graph(anchor.frame(delta));
        }
        self.anchor = Some(anchor);
    }

    /// Advance one frame, pushing this frame's `GraphIntent::MoveSelection`
    /// while the drag is held.
    ///
    /// A cancelling Esc takes the drag's undo entry back whole, which puts
    /// every member back where the drag latched it, and ends the drag; the
    /// gesture records nothing. `darkroom/AGENTS.md`, "Undo", "A held gesture
    /// is one entry, and Esc takes it back whole".
    ///
    /// Runs pre-record, so the move lands in `Document` before the pass that
    /// draws the moved items: they paint at the cursor in Pass A with no
    /// relayout retry.
    pub(crate) fn input(&mut self, stage: &mut InputStage<'_, '_, UnsettledCtx<'_>>) {
        let ctx = stage.ctx;
        let graph_ctx = ctx.graph_ctx();
        // An item deleted mid-drag (breaker swipe, undo) takes its drag with
        // it: left in place the anchor would emit against a missing item.
        if self
            .anchor
            .as_ref()
            .is_some_and(|anchor| !graph_ctx.view().holds(anchor.grab.item))
        {
            self.anchor = None;
        }
        let Some(anchor) = &self.anchor else {
            return;
        };
        let resp = stage.update.input.response_for(anchor.grab.handle);
        // `drag_started` on a still-active anchor means a *new* gesture just
        // latched on the same widget. Emitting with the stale start
        // positions would snap the group back to the previous gesture's
        // start point; the caller's latch scan picks the new one up instead.
        if resp.left.drag.started() {
            self.anchor = None;
            return;
        }
        if ctx.cancelled() {
            // The gesture's whole entry goes back, a spawn it joined too; should an edit in
            // between have sealed it, the members go back to where the drag latched them.
            stage
                .out
                .push_cancel(anchor.gesture, anchor.frame(Vec2::ZERO));
            self.anchor = None;
            return;
        }
        // No delta means the drag isn't latched anymore — release, or the
        // pointer left the surface.
        let Some(delta) = resp.left.drag.delta() else {
            self.anchor = None;
            return;
        };
        // Palantir reports drag deltas in the widget's pre-transform frame,
        // which is the same canvas-world space item positions live in.
        stage.out.push_graph(anchor.frame(delta));
    }
}

impl Anchor {
    /// The frame of this drag that puts every member at its start plus
    /// `offset`.
    fn frame(&self, offset: Vec2) -> GraphIntent {
        GraphIntent::MoveSelection {
            gesture: self.gesture,
            members: Rc::clone(&self.grab.members),
            offset,
        }
    }
}

/// The group drag of `node`, pressed on its `handle`: with the selection when the node is in it,
/// else alone, selected, with the ties inside it.
pub(crate) fn node_grab(node: NodeCtx<'_>, handle: WidgetId) -> GrabStart {
    let graph_ctx = node.graph_ctx;
    let item = ItemId::Node(node.node_id);
    let (members, joined) = if graph_ctx.is_selected(item) {
        (selected_group(graph_ctx), Vec::new())
    } else {
        let alone = vec![DragStart {
            item,
            pos: node.pos,
        }];
        let selects = GraphIntent::click(ClickMode::Replace, graph_ctx.selected(), item);
        (with_inner_ties(graph_ctx.view(), alone), selects.collect())
    };
    GrabStart {
        grab: Grab {
            item,
            members,
            handle,
        },
        joined,
    }
}

/// The selection's members at their current positions, with the ties inside
/// its nodes — [`GroupDrag::latch`]'s `members` for a drag that grabbed an
/// already-selected member.
pub(crate) fn selected_group(graph_ctx: GraphCtx<'_>) -> Rc<[DragStart]> {
    let view = graph_ctx.view();
    let nodes = graph_ctx
        .nodes()
        .filter(|n| graph_ctx.is_selected(ItemId::Node(n.node_id)))
        .map(|n| DragStart {
            item: ItemId::Node(n.node_id),
            pos: n.pos,
        });
    let ties = view
        .ties
        .iter()
        .filter(|(tie, _)| graph_ctx.is_selected(ItemId::Tie(**tie)))
        .map(|(tie, placed)| DragStart {
            item: ItemId::Tie(*tie),
            pos: placed.pos,
        });
    with_inner_ties(view, nodes.chain(ties).collect())
}

/// `members`, and after them each tie whose every wire has both end nodes
/// among the member nodes, at its current position: a drag of a node set
/// takes the ties inside it. `darkroom/AGENTS.md`, "Wire ties", "A node set takes the
/// ties inside it", holds the decision.
pub(crate) fn with_inner_ties(view: &GraphView, mut members: Vec<DragStart>) -> Rc<[DragStart]> {
    let nodes: BTreeSet<NodeId> = members
        .iter()
        .filter_map(|member| member.item.node())
        .collect();
    for tie in view.ties_within(|node_id| nodes.contains(&node_id)) {
        let item = ItemId::Tie(tie);
        if !members.iter().any(|member| member.item == item) {
            members.push(DragStart {
                item,
                pos: view.ties[&tie].pos,
            });
        }
    }
    members.into()
}

#[cfg(test)]
mod tests;
