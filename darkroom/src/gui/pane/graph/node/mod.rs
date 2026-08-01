pub(super) mod header;
mod memory_row;
pub(super) mod port_color;
pub(crate) mod port_row;
pub(crate) mod preview_row;
mod value_editor;
pub(super) mod widget;

use crate::core::document::PortRef;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::gesture::breaker::BreakerProbe;
use crate::gui::pane::graph::gesture::drag_anchor::GroupDrag;
use crate::gui::requests::Requests;
use palantir::{Track, Ui, WidgetId};
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
    /// The body/title drag, latched in `draw_one` and stepped by
    /// [`Self::prepass`].
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
    /// Drop the in-flight drag and the focus hysteresis. `row_tracks` is
    /// scratch grown to the widest node seen, not gesture state, so it keeps
    /// its capacity across the reset.
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
        for n in dcx.graph_ctx().nodes_in_paint_order() {
            let keeps_focus = ui.focus_within(node_widget_id(n.id));
            if keeps_focus {
                focus_kept = Some(n.id);
            }
            if !dcx.cull().keeps_node(dcx.geometry().node_world_rect(n))
                && !keeps_focus
                && self.focus_kept_last != Some(n.id)
            {
                continue;
            }
            let ncx = n.with_hover(node_hovered(ui, n.id));
            let node = widget::NodeWidget::new(self, ncx).show(ui, dcx, probe, out);
            if node.inspect_toggled {
                outcome.inspect_toggled.get_or_insert(n.id);
            }
            outcome.body_acted |= node.body_acted;
            if node.menu_opened {
                outcome.menu_opened.get_or_insert(n.id);
            }
        }
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

/// A node-keyed widget id. Every id in a node's subtree is reconstructible
/// from its domain coordinates, so a scan can `response_for` last frame's
/// state for any of them without threading a cache — which is what lets the
/// prepass read clicks before the record.
pub(super) fn node_wid(tag: &'static str, node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node", tag, node_id))
}

/// A port-keyed widget id — [`node_wid`] for the per-port widgets, keyed by
/// side and index as well.
pub(super) fn port_wid(tag: &'static str, port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node",
        tag,
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// The node's outer body panel — probed by the connection breaker for last
/// frame's arranged rect, before the panel's own response round-trips.
pub(super) fn node_widget_id(node_id: NodeId) -> WidgetId {
    node_wid("body", node_id)
}

/// Every widget whose drag moves `node_id`'s body, in the order
/// [`NodeUI::draw_one`] tries them.
///
/// Not just the body panel: the header title doubles as a drag handle —
/// its idle label senses `DRAG` and **swallows the press**, so the body
/// never sees one latched there. (While renaming, the title is a
/// `TextEdit` with no `DRAG`, so it can't fire mid-edit.)
///
/// Deliberately *curated*, not "everything under the node". Port
/// circles, header chips, and the inline value editors are all inside
/// the body panel and all latch a drag of their own once the pointer
/// travels — palantir's drag latch ignores `Sense`, so even a
/// `Sense::CLICK` port circle reports one. Each of those owns its own
/// gesture (a wire, a chip click, a text drag), so a subtree-wide
/// "did anything in here start a drag" would wrongly move the node
/// along with them. The dock's tab chips carry the same shape for the
/// same reason (`gui::dock::strip::drag_handles`).
fn drag_handles(node_id: NodeId) -> impl Iterator<Item = WidgetId> {
    [node_widget_id(node_id), node_rename_wid(node_id)].into_iter()
}

/// Pointer-over-node for hover-reveal affordances (the value-editor
/// chips). The body response's own `hovered` flag misses most of the
/// node's area — ports, chips, and editors capture the hit — so this
/// asks whether the hover *target* sits anywhere in the node's subtree.
/// Target-derived (not a raw `pointer_pos` rect test) on purpose: it
/// can only change when the hover target changes, which is exactly when
/// a repaint is already scheduled — no `MOVE` subscription needed — and
/// it's occlusion-aware (a panel stacked over the node wins the
/// pointer).
///
/// Resolved against *last* frame's hover target and cascade, so the answer
/// doesn't depend on where in this frame's record it is asked — which is what
/// lets the record pass settle it at the node body — one
/// [`NodeCtx::with_hover`](crate::gui::graph_ctx::node_ctx::NodeCtx::with_hover)
/// per node — before the subtree that reads it has recorded.
pub(super) fn node_hovered(ui: &Ui, node_id: NodeId) -> bool {
    ui.hover_within(node_widget_id(node_id))
}

/// A node's inline title-rename editor *and* its idle label, so the same id
/// is recorded across the label⇄editor swap. Polled here to drag the node by
/// its title (the idle label senses `DRAG`).
fn node_rename_wid(node_id: NodeId) -> WidgetId {
    node_wid("title_rename", node_id)
}
