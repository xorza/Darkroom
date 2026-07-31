pub(crate) mod ctx;
pub(super) mod header;
mod memory_row;
pub(super) mod port_color;
pub(super) mod port_row;
pub(crate) mod preview_row;
mod value_editor;

use crate::core::document::PortRef;
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::gesture::breaker::BreakerProbe;
use crate::gui::pane::graph::gesture::drag_anchor::GroupDrag;
use crate::gui::pane::graph::gesture::drag_anchor::selected_group_positions;
use crate::gui::pane::graph::node::ctx::NodeCtx;
use crate::gui::pane::graph::node::header::{header, status_row, subscription_pin};
use crate::gui::pane::graph::node::memory_row::memory_row;
use crate::gui::pane::graph::node::port_row::ports_row;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use glam::Vec2;
use palantir::{
    Background, Color, Configure, Corners, Panel, Sense, Shadow, Sizing, Stroke, Track, Ui,
    WidgetId,
};
use scenarium::Binding;
use scenarium::InputPort;
use scenarium::NodeId;
use std::collections::BTreeSet;

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

impl NodeUI {
    /// Record the widget tree of every scene node retained by `cull`
    /// (plus the focus-owning node — see the loop comment),
    /// skipping off-screen ones entirely. Emits selection/raise intents
    /// for body clicks and latches the drag anchor for a body/title drag
    /// (port circles capture their own presses via `Sense::CLICK`, so
    /// drags don't latch off the port grabs); `prepass` converts the
    /// anchor into `GraphIntent::MoveSelection` on later frames.
    pub(super) fn draw_all(
        &mut self,
        ui: &mut Ui,
        dcx: DrawCtx<'_>,
        probe: &mut BreakerProbe<'_>,
        out: &mut Intents,
    ) {
        // Paint in the context's node order (the view's `item_placements`) —
        // later draws sit on top, so the last item is frontmost. The order is
        // persisted view state, so a raised item stays raised across
        // save/load and tab switches; `GraphIntent::Raise` moves a clicked
        // item to the end.
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
        let mut focus_kept = None;
        for n in dcx.graph_ctx().nodes() {
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
            self.draw_one(ui, NodeCtx::for_node(dcx, ui, n), probe, out);
        }
        self.focus_kept_last = focus_kept;
        // Belt-and-braces against a node deleted mid-drag; `prepass` makes
        // the same check before it can emit anything against it.
        self.drag.drop_if_owner_gone(dcx.graph_ctx());
    }

    fn draw_one(
        &mut self,
        ui: &mut Ui,
        ncx: NodeCtx<'_>,
        probe: &mut BreakerProbe<'_>,
        out: &mut Intents,
    ) {
        let (theme, node) = (ncx.theme(), ncx.node());

        // Probe the body against the breaker polyline. Hit → recolor border
        // red and flag the node for deletion on release. The rect is the same
        // `node_world_rect` the cull above and the rubber band test — this
        // frame's position plus the cached measured size — so all three agree
        // on where the node is even when the document moved it out from under
        // a live gesture (an undo, say). A node that has never
        // recorded has no size yet, so the breaker can't catch it until next
        // frame: acceptable, since the user can't aim at something unpainted.
        let broken = ncx
            .geometry()
            .node_world_rect(node)
            .is_some_and(|r| probe.crosses_rect(r));
        if broken {
            probe.mark_broken_node(node.id);
        }
        let selected = ncx.is_selected();
        // The border width is *always* the selection width so selecting a
        // node never resizes it (stroke folds into padding — width-gated,
        // not color-gated). Only the color changes, a 4-tier decision: the
        // breaker alarm wins, then the missing-stub color, then
        // `Theme::card_border`'s own broken/selected/resting 3-tier (broken
        // can't recur here since it's already handled, but the helper still
        let border_width = theme.card_border_width();
        let border = if node.missing() && !broken {
            // A stub for a node whose func is gone from the library: paint it
            // in the error color so it reads as broken-but-deletable.
            theme.colors.exec_errored_glow
        } else {
            theme.card_border(broken, selected).color
        };
        // Sample modifiers before the panel borrows `ui` for the rest
        // of this scope (the click handler below can't reborrow it).
        let shift_click = ui.modifiers().shift;
        // Status glow when the node ran, else the ambient elevation shadow —
        // one slot, and live status outranks depth.
        let shadow = node_shadow(theme, node.exec_status());

        // The subscription pin records just before the body so it peeks out
        // from behind this node's corner while riding the same cull decision
        // and stack position as the node itself.
        if node.sink() {
            subscription_pin(ui, theme, node, ncx.geometry().subs.is_hovered(node.id));
        }

        // Borrowed off `self` before the body closure so it can't conflict
        // with the drag latch below, which reads a different field.
        let row_tracks = &mut self.row_tracks;
        let panel = Panel::vstack()
            .id(node_widget_id(node.id))
            .position(node.pos)
            // A preview needs room for a thumbnail; every other node keeps the
            // theme's own floor.
            .min_size(if node.preview() {
                (preview_row::PREVIEW_MIN_WIDTH, theme.node_min_height)
            } else {
                (theme.node_min_width, theme.node_min_height)
            })
            .size((Sizing::HUG, Sizing::HUG))
            .sense(Sense::CLICK | Sense::DRAG)
            .background(
                Background::rounded(
                    theme.colors.node_fill,
                    Corners::all(theme.node_corner_radius),
                )
                .with_stroke(Stroke::solid(border, border_width))
                .with_shadow(shadow),
            )
            .show(ui, |ui| {
                header(ui, ncx, out);
                status_row(ui, ncx, out);
                ports_row(ui, ncx, row_tracks, out);
                // A preview has no output, so it has no cached value for the
                // memory readout to report — its value takes that slot instead.
                if node.preview() {
                    preview_row::preview_row(ui, ncx);
                } else {
                    memory_row(ui, ncx);
                }
            });
        // Pull the body response's click flag into a local so its `&Ui`
        // borrow ends before the handle scan below. (`Response` is a lazy
        // probe over `response_for`, so reading the body through either is
        // the same last-frame state.)
        let body_clicked = panel.response.left.clicked();

        // Click without drag → select. Plain click selects only this
        // node; Shift-click toggles its membership in the current
        // selection. `UndoStep::is_noop` filters a click that doesn't
        // change the set (e.g. clicking the sole selected node).
        if body_clicked {
            click_intents(shift_click, ncx.graph_ctx(), node.id, out);
        }

        // Latch the anchor on the press-frame edge, off whichever handle
        // caught the press (resolved by this frame's sweep, which walks the
        // same curated `drag_handles` list); subsequent frames' `prepass`
        // peeks `response_for(widget_id)` before record runs and converts
        // `drag_delta` into a `MoveSelection` applied to `Document` before
        // the record reads it back.
        if let Some(handle) = ncx.hits().latched_on(node.id) {
            // Grabbing a node already in the selection drags the whole
            // group together;
            // grabbing an unselected node selects only it and drags it
            // alone.
            let start_positions = if selected {
                selected_group_positions(ncx.draw_ctx())
            } else {
                click_intents(false, ncx.graph_ctx(), node.id, out);
                vec![(node.id, node.pos)]
            };
            self.drag.latch(node.id, start_positions, handle);
        }
    }

    /// Pre-record pass: peek palantir's input state for any widgets
    /// this `NodeUI` owns and push the corresponding `GraphIntent`s into
    /// `out`. Runs in the pre-record pass, so any state mutation applied
    /// from these intents (notably drag-driven `MoveSelection`) lands in
    /// `Document` before recording — Pass A's arrange already reflects the
    /// cursor; no Pass B relayout retry.
    pub(super) fn prepass(&mut self, ui: &Ui, graph_ctx: GraphCtx<'_>, out: &mut Intents) {
        self.drag.advance(ui, graph_ctx, out);
    }
}

/// The accent color for a node's last-run status, or `None` when it
/// didn't run. Shared by the body glow and the header time label so they
/// read as one cue.
pub(super) fn exec_color(theme: &Theme, status: ExecStatus) -> Option<Color> {
    match status {
        ExecStatus::None => None,
        ExecStatus::Cached => Some(theme.colors.exec_cached_glow),
        ExecStatus::Executed(_) => Some(theme.colors.exec_executed_glow),
        ExecStatus::Running(_) => Some(theme.colors.exec_running_glow),
        ExecStatus::MissingInputs => Some(theme.colors.exec_missing_glow),
        ExecStatus::Errored => Some(theme.colors.exec_errored_glow),
    }
}

/// The node body's one shadow: the status glow for its last-run outcome
/// (zero offset so the halo wraps evenly), or — when it didn't run — a soft
/// ambient drop shadow that lifts the body off the canvas and the wires
/// crossing beneath it. The ambient color is the theme's elevation swatch
/// (`node_ambient_shadow`), shared with the inspector panels so all
/// elevated surfaces cast one kind of shadow.
fn node_shadow(theme: &Theme, status: ExecStatus) -> Shadow {
    match exec_color(theme, status) {
        // Blur/spread sized so the glow carries elevation too — it replaces
        // the ambient shadow, and a tighter halo would leave a just-run node
        // sitting flatter than its idle neighbors. Kept a touch tighter than
        // the ambient shadow so the status reads as a crisp halo, not a bloom.
        Some(color) => Shadow::drop(color, Vec2::ZERO, 3.0).with_spread(0.5),
        None => theme.elevation_shadow(10.0),
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
pub(crate) fn drag_handles(node_id: NodeId) -> impl Iterator<Item = WidgetId> {
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
/// lets [`NodeCtx::for_node`](crate::gui::pane::graph::node::ctx::NodeCtx::for_node) settle
/// it at the node body, before the subtree
/// that reads it has recorded.
pub(super) fn node_hovered(ui: &Ui, node_id: NodeId) -> bool {
    ui.hover_within(node_widget_id(node_id))
}

/// A node's inline title-rename editor *and* its idle label, so the same id
/// is recorded across the label⇄editor swap. Polled here to drag the node by
/// its title (the idle label senses `DRAG`).
fn node_rename_wid(node_id: NodeId) -> WidgetId {
    node_wid("title_rename", node_id)
}

pub(super) fn set_input(port: PortRef, to: impl Into<Option<Binding>>) -> GraphIntent {
    GraphIntent::SetInput {
        input: InputPort::new(port.node_id, port.port_idx),
        to: to.into(),
    }
}

/// The intents a click on `key` produces: the selection change plus a lift
/// to the top of the paint stack, so clicking a node body
/// preview brings it to the front. The raise is skipped only when a
/// Shift-click *removes* the item from the selection — an item you just
/// deselected shouldn't jump forward. Shared by the node body, header
/// title, and port labels so clicking any of them behaves like clicking the
/// body.
pub(super) fn click_intents(shift: bool, graph_ctx: GraphCtx<'_>, key: NodeId, out: &mut Intents) {
    out.push(select_intent(shift, graph_ctx, key));
    let deselecting = shift && graph_ctx.is_selected(key);
    if !deselecting {
        out.push(GraphIntent::Raise { key });
    }
}

/// The `SetSelection` a click on `key` produces: plain click selects only
/// it, Shift-click toggles its membership. `UndoStep::is_noop` drops the
/// entry when nothing changed.
fn select_intent(shift: bool, graph_ctx: GraphCtx<'_>, key: NodeId) -> GraphIntent {
    let mut to = if shift {
        graph_ctx.selected().clone()
    } else {
        BTreeSet::new()
    };
    if shift && graph_ctx.is_selected(key) {
        to.remove(&key);
    } else {
        to.insert(key);
    }
    GraphIntent::SetSelection { to }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::gui::graph_ctx::internals::GraphCtxFixture;

    /// A graph holding `selected` as both its node set and its committed
    /// selection — enough for the click-intent rules, which read nothing
    /// else.
    fn scene_with_selection(selected: impl IntoIterator<Item = NodeId>) -> GraphCtxFixture {
        let ids: Vec<NodeId> = selected.into_iter().collect();
        GraphCtxFixture::with_nodes(ids.iter().map(|id| (*id, Vec2::ZERO)))
            .with_selection(ids.iter().copied())
    }

    fn click(shift: bool, scene: &mut GraphCtxFixture, id: NodeId) -> Vec<GraphIntent> {
        use crate::core::edit::intent::sink::Queued;

        let mut out = Intents::default();
        click_intents(shift, scene.graph_ctx(), id, &mut out);
        out.drain()
            .map(|queued| match queued {
                Queued::Graph(intent) => intent,
                Queued::Dock(intent) => panic!("a node click raises nothing global: {intent:?}"),
            })
            .collect()
    }

    #[test]
    fn click_intents_raises_unless_shift_deselects() {
        let a = NodeId::unique();
        let b = NodeId::unique();

        // Plain click on an unselected node: select it, then raise it.
        let out = click(false, &mut scene_with_selection([]), a);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], GraphIntent::SetSelection { .. }));
        assert!(matches!(out[1], GraphIntent::Raise { key } if key == a));

        // Plain click on an already-selected node still raises it.
        let out = click(false, &mut scene_with_selection([a]), a);
        assert!(
            out.iter()
                .any(|i| matches!(i, GraphIntent::Raise { key } if *key == a)),
            "a plain click always lifts its node to the front"
        );

        // Shift-click adding a fresh node to the selection raises it.
        let out = click(true, &mut scene_with_selection([a]), b);
        assert!(
            out.iter()
                .any(|i| matches!(i, GraphIntent::Raise { key } if *key == b)),
            "shift-adding a node raises it"
        );

        // Shift-click removing a node does NOT raise it — a node you just
        // deselected shouldn't jump to the front.
        let out = click(true, &mut scene_with_selection([a, b]), b);
        assert_eq!(out.len(), 1, "shift-deselect suppresses the raise");
        assert!(matches!(out[0], GraphIntent::SetSelection { .. }));
    }
}
