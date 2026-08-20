pub(super) mod header;
mod memory_row;
pub(super) mod port_color;
pub(crate) mod port_row;
pub(crate) mod preview_row;
mod value_editor;
pub(super) mod wid;
pub(super) mod widget;

use crate::core::document::StackedItem;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::gesture::breaker::BreakerProbe;
use crate::gui::pane::graph::gesture::drag_anchor::GroupDrag;
use crate::gui::requests::Requests;
use palantir::{Track, Ui};
use scenarium::NodeId;

/// Owns rendering of every graph node plus the single active drag
/// anchor — the press-frame positions are snapshotted here so each
/// `MoveSelection` target is `start_pos + drag_delta`, not a running
/// integration over the moving source. Only one node can hold the
/// pointer at a time, so one anchor slot is enough.
///
/// `draw_all` is the single entry point; `GraphUI` calls it once per
/// frame after [`crate::gui::pane::graph::frame::geometry::CanvasGeometry`] has been rebuilt
/// from last-frame's responses.
#[derive(Default, Debug)]
pub(super) struct NodeUI {
    /// The body/title drag, latched in [`widget::NodeWidget::show`] and
    /// stepped by [`Self::prepass`].
    drag: GroupDrag,
    /// The node kept recorded by the focus cull-exemption last frame.
    /// Focus clears during input, *before* the record, so on the blur
    /// frame `focus_within` is already false — but that frame is exactly
    /// when an in-progress const edit commits (the editor's pending draft
    /// resolves on its first post-blur record). One frame of hysteresis
    /// keeps the node recorded through it; otherwise the cull would let
    /// palantir sweep the draft unseen.
    focus_kept_last: Option<NodeId>,
    /// Row tracks staged for the port grids, grown to the widest node seen.
    ///
    /// `Grid::show` copies its tracks into the tree's own capacity-retained
    /// arena, so this buffer only has to live for the length of that call —
    /// but building it fresh meant an allocation per node per frame, for a
    /// run of identical tracks that is memcpy'd out and dropped. Every row of
    /// every node takes the same track, so one buffer serves the whole frame
    /// and each node slices the prefix it needs.
    row_tracks: Vec<Track>,
    /// The draw sweep's back-to-front node order, grown to the largest graph
    /// seen. Sized to the *whole* graph — the sweep resolves stacking before
    /// it can cull — so a fresh `Vec` per frame would scale its allocation
    /// with the document.
    paint_order: Vec<StackedItem>,
}

/// What one record pass's node draw saw but could not act on itself.
///
/// Each drives state the draw itself holds *shared* — the inspection panels
/// through [`DrawCtx`] so they can paint, the context menu because it is the
/// canvas's and not a node's. So the clicks are seen here and applied by the
/// caller once the draw is over and both can be taken `&mut`.

#[derive(Default, Debug)]
pub(crate) struct NodeDrawOutcome {
    /// The node whose `i` chip was clicked, cycling its inspection panel.
    pub(crate) inspect_toggled: Option<NodeId>,
    /// Whether any node body was clicked or started a drag.
    pub(crate) body_acted: bool,
    /// The node whose body was right-clicked, opening its context menu.
    pub(crate) menu_opened: Option<NodeId>,
}

impl NodeUI {
    /// Drop the in-flight drag and the focus hysteresis. `row_tracks` and
    /// `paint_order` are scratch grown to the largest scene seen, not gesture
    /// state, so they keep their capacity across the reset.
    pub(super) fn reset(&mut self) {
        self.drag.reset();
        self.focus_kept_last = None;
    }

    /// Record the widget tree of every scene node retained by `cull`
    /// (plus the focus-owning node — see the loop comment),
    /// skipping off-screen ones entirely. Emits selection/raise intents
    /// for body clicks and latches the drag anchor for a body/title drag
    /// (port circles capture their own presses via `Sense::CLICK`, so
    /// drags don't latch off the port grabs); `prepass` converts the
    /// anchor into `GraphIntent::MoveSelection` on later frames.
    /// Record every node the cull keeps, and report what the draw saw but
    /// could not act on — see [`NodeDrawOutcome`].
    pub(super) fn draw_all(
        &mut self,
        ui: &mut Ui,
        dcx: DrawCtx<'_>,
        probe: &mut BreakerProbe<'_>,
        out: &mut Requests,
    ) -> NodeDrawOutcome {
        // Paint back-to-front, so the last item drawn is frontmost. Each
        // item's depth is persisted view state, so a raised node stays raised
        // across save/load and tab switches; `GraphIntent::Raise` lifts a
        // clicked item past the rest.
        //
        // Culled nodes are skipped entirely — no measure, arrange, or
        // paint. Every widget id in a node's subtree derives from its
        // `NodeId` (explicit `from_hash` ids, and palantir resolves auto ids
        // parent-scoped under them), so culling a sibling can't re-key
        // anything that stays on screen. Palantir *does* drop widget state
        // for ids not recorded this frame, so a node whose subtree holds the
        // keyboard focus (`focus_within` — an in-progress title/const/port
        // edit) stays recorded even off-screen; otherwise panning away
        // mid-edit would discard the draft. The exemption carries one frame
        // past the blur (`focus_kept_last`): focus clears before the record,
        // and that first post-blur record is where the edit's pending draft
        // commits.
        let mut outcome = NodeDrawOutcome::default();
        let mut focus_kept = None;
        // Swapped out for the sweep because each node hands the whole `NodeUI`
        // to its widget; it goes back below with its capacity, so only a graph
        // larger than every earlier one ever allocates here.
        let mut order = std::mem::take(&mut self.paint_order);
        let graph_ctx = dcx.graph_ctx();
        graph_ctx.paint_order(&mut order);
        for item in &order {
            // A placement whose node is gone — deleted since the order was
            // taken — resolves to nothing and is skipped, not faked.
            let Some(n) = graph_ctx.node(item.id) else {
                continue;
            };
            let keeps_focus = ui.focus_within(wid::body(n.id));
            if keeps_focus {
                focus_kept = Some(n.id);
            }
            if !dcx.cull().keeps_node(dcx.geometry().node_world_rect(n))
                && !keeps_focus
                && self.focus_kept_last != Some(n.id)
            {
                continue;
            }
            let ncx = n.with_hover(wid::hovered(ui, n.id));
            let node = widget::NodeWidget::new(self, ncx).show(ui, dcx, probe, out);
            if node.inspect_toggled {
                outcome.inspect_toggled.get_or_insert(n.id);
            }
            outcome.body_acted |= node.body_acted;
            if node.menu_opened {
                outcome.menu_opened.get_or_insert(n.id);
            }
        }
        self.paint_order = order;
        self.focus_kept_last = focus_kept;
        // Belt-and-braces against a node deleted mid-drag; `prepass` makes
        // the same check before it can emit anything against it.
        self.drag.drop_if_owner_gone(dcx.graph_ctx());
        outcome
    }

    /// Pre-record pass: peek palantir's input state for any widgets
    /// this `NodeUI` owns and push the corresponding `GraphIntent`s into
    /// `out`. Runs in the pre-record pass, so any state mutation applied
    /// from these intents (notably drag-driven `MoveSelection`) lands in
    /// `Document` before recording — Pass A's arrange already reflects the
    /// cursor; no Pass B relayout retry.
    pub(super) fn prepass(&mut self, ui: &Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        self.drag.advance(ui, graph_ctx, out);
    }
}
