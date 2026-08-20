//! One node's body: the widget that records it, and what recording it reports.

use glam::Vec2;
use palantir::{Background, Color, Configure, Corners, Panel, Sense, Shadow, Sizing, Stroke, Ui};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::gesture::breaker::BreakerProbe;
use crate::gui::pane::graph::gesture::drag_anchor::selected_group_positions;
use crate::gui::pane::graph::node::header::{header, status_row, subscription_pin};
use crate::gui::pane::graph::node::memory_row::memory_row;
use crate::gui::pane::graph::node::port_row::ports_row;
use crate::gui::pane::graph::node::{NodeUI, preview_row, wid};
use crate::gui::requests::Requests;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;

/// What drawing **one** node reports back: the signals its own widgets
/// produced but could not act on, in the shape palantir's
/// `TextEditResponse` uses — the drawer is authoritative about what it did
/// with this frame's input, so no caller re-polls for it.
///
/// Booleans, because this is one node. Which node across the scene is
/// [`NodeDrawOutcome`](crate::gui::pane::graph::node::NodeDrawOutcome)'s
/// question, and folding these into it is the loop's
/// job rather than the node's.
#[derive(Default, Debug)]
pub(super) struct NodeResponse {
    pub(super) inspect_toggled: bool,
    pub(super) body_acted: bool,
    pub(super) menu_opened: bool,
}

/// One node's body, as a widget over the state every node on the canvas
/// shares.
///
/// A builder in palantir's shape — `new(..).show(ui, ..)` — but taking its
/// state `&mut` rather than keeping it in `ui.state_mut()`, because none of
/// what [`NodeUI`] holds is per-node: there is one pointer so one drag, and
/// `row_tracks` is deliberately one buffer the whole frame slices out of.
/// Per-widget state would reintroduce the allocation it exists to avoid.
pub(super) struct NodeWidget<'a> {
    state: &'a mut NodeUI,
    ncx: NodeCtx<'a>,
}

impl<'a> NodeWidget<'a> {
    pub(super) fn new(state: &'a mut NodeUI, ncx: NodeCtx<'a>) -> Self {
        Self { state, ncx }
    }

    pub(super) fn show(
        self,
        ui: &mut Ui,
        dcx: DrawCtx<'_>,
        probe: &mut BreakerProbe<'_>,
        out: &mut Requests,
    ) -> NodeResponse {
        let Self { state, ncx } = self;
        let (theme, node) = (ncx.theme(), ncx);

        // Probe the body against the breaker polyline. Hit → recolor border
        // red and flag the node for deletion on release. The rect is the same
        // `node_world_rect` the cull above and the rubber band test — this
        // frame's position plus the cached measured size — so all three agree
        // on where the node is even when the document moved it out from under
        // a live gesture (an undo, say). A node that has never
        // recorded has no size yet, so the breaker can't catch it until next
        // frame: acceptable, since the user can't aim at something unpainted.
        let broken = dcx
            .geometry()
            .node_world_rect(node)
            .is_some_and(|r| probe.crosses_rect(r));
        if broken {
            probe.mark_broken_node(node.id);
        }
        let selected = dcx.is_selected(ncx.id);
        // The border width is *always* the selection width so selecting a
        // node never resizes it (stroke folds into padding — width-gated,
        // not color-gated). Only the color changes, a 4-tier decision: the
        // breaker alarm wins, then the missing-stub color, then
        // `Theme::card_border`'s own broken/selected/resting 3-tier (broken
        // can't recur here since it's already handled, but the helper still
        let border_width = theme.card.border_width_total();
        let border = if node.missing() && !broken {
            // A stub for a node whose func is gone from the library: paint it
            // in the error color so it reads as broken-but-deletable.
            theme.status.error
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
            subscription_pin(ui, theme, node, dcx.geometry().subs.is_hovered(node.id));
        }

        // Borrowed off `self` before the body closure so it can't conflict
        // with the drag latch below, which reads a different field.
        let row_tracks = &mut state.row_tracks;
        let panel = Panel::vstack()
            .id(wid::body(node.id))
            .position(node.pos)
            // A preview needs room for a thumbnail; every other node keeps the
            // theme's own floor.
            .min_size(if node.preview() {
                (preview_row::PREVIEW_MIN_WIDTH, theme.card.min_height)
            } else {
                (theme.card.min_width, theme.card.min_height)
            })
            .size((Sizing::HUG, Sizing::HUG))
            .sense(Sense::CLICK | Sense::DRAG)
            .background(
                Background::rounded(theme.card.fill, Corners::all(theme.card.corner_radius))
                    .with_stroke(Stroke::solid(border, border_width))
                    .with_shadow(shadow),
            )
            .show(ui, |ui| {
                let inspect_toggled = header(ui, ncx, dcx, out);
                status_row(ui, ncx, out);
                ports_row(ui, ncx, dcx, row_tracks, out);
                // A preview has no output, so it has no cached value for the
                // memory readout to report — its value takes that slot instead.
                if node.preview() {
                    preview_row::preview_row(ui, ncx, out);
                } else {
                    memory_row(ui, ncx);
                }
                inspect_toggled
            });
        // Pull the body response's flags into locals so its `&Ui` borrow ends
        // before the handle scan below. (`Response` is a lazy probe over
        // `response_for`, so reading the body through either is the same
        // last-frame state.)
        //
        // `body_acted` is "the user acted on a node", as opposed to on the
        // bare canvas — what closes the unpinned inspection panels. The title
        // is deliberately not folded in: a title drag moves the node but has
        // never counted.
        let body_clicked = panel.response.left.clicked();
        let response = NodeResponse {
            inspect_toggled: panel.inner,
            body_acted: body_clicked || panel.response.left.drag.started(),
            menu_opened: panel.response.right.clicked(),
        };

        // Click without drag → select. Plain click selects only this
        // node; Shift-click toggles its membership in the current
        // selection. `UndoStep::is_noop` filters a click that doesn't
        // change the set (e.g. clicking the sole selected node).
        if body_clicked {
            out.extend_graph(GraphIntent::click(
                shift_click,
                ncx.graph_ctx.selected(),
                node.id,
            ));
        }

        // Latch the anchor on the press-frame edge, off whichever of this
        // node's handles caught the press. Read here, where the handles are
        // built, over the same curated list they are drawn from; subsequent
        // frames' `prepass` peeks `response_for(widget_id)` before record runs
        // and converts `drag_delta` into a `MoveSelection` applied to
        // `Document` before the record reads it back.
        if let Some(handle) =
            wid::drag_handles(node.id).find(|w| ui.response_for(*w).left.drag.started())
        {
            // Grabbing a node already in the selection drags the whole
            // group together;
            // grabbing an unselected node selects only it and drags it
            // alone.
            let start_positions = if selected {
                selected_group_positions(dcx)
            } else {
                out.extend_graph(GraphIntent::click(false, ncx.graph_ctx.selected(), node.id));
                vec![(node.id, node.pos)]
            };
            state.drag.latch(node.id, start_positions, handle);
        }
        response
    }
}

/// The accent color for a node's last-run status, or `None` when it
/// didn't run. Shared by the body glow and the header time label so they
/// read as one cue.
pub(crate) fn exec_color(theme: &Theme, status: ExecStatus) -> Option<Color> {
    match status {
        ExecStatus::None => None,
        ExecStatus::Cached => Some(theme.status.info),
        ExecStatus::Executed(_) => Some(theme.status.success),
        ExecStatus::Running(_) => Some(theme.status.busy),
        ExecStatus::MissingInputs => Some(theme.status.warning),
        ExecStatus::Errored => Some(theme.status.error),
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
        None => theme.card.elevation_shadow(10.0),
    }
}
