mod card;
mod const_editor;
pub(crate) mod header;
mod node_footer;
pub(super) mod node_record;
pub(crate) mod port_row;
pub(crate) mod preview_row;
pub(crate) mod run_status;

use std::mem;

use palantir::{Border, Configure, Panel, Sense, Shadow, Sizing, Track, Ui, Visibility};
use scenarium::NodeId;

use crate::core::document::graph_view::StackedNode;
use crate::core::document::item_id::ItemId;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::gesture::breaker_ui::breaker_probe::BreakerProbe;
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::wid;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::hover::Hover;
use crate::gui::theme::{CardTier, Theme};

/// Owns rendering of every graph node.
///
/// `draw_all` is the single entry point; `GraphUi` calls it once per frame after
/// [`crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry`] has been rebuilt from
/// last-frame's responses.
#[derive(Default, Debug)]
pub(super) struct NodeUi {
    /// The node kept recorded by the focus cull-exemption last frame. Focus clears during
    /// input, before the record, so on the blur frame `is_focus_within` is already false, yet
    /// that is when an in-progress const edit commits (its pending draft resolves on the first
    /// post-blur record). One frame of hysteresis keeps the node recorded through it.
    focus_kept_last: Option<NodeId>,
    /// Row tracks staged for the port grids, grown to the widest node seen. `Grid::show` copies
    /// its tracks into the tree's own arena, and every row of every node takes the same track,
    /// so one buffer serves the whole frame and each node slices the prefix it needs.
    row_tracks: Vec<Track>,
    /// The draw sweep's back-to-front node order, grown to the largest graph seen. Sized to the
    /// whole graph, as the sweep resolves stacking before it can cull.
    paint_order: Vec<StackedNode>,
}

/// The canvas pass every node body records in: its stage, whose requests their widgets raise edits
/// into, and the breaker probe the bodies mark hits against.
#[derive(Debug)]
pub(super) struct NodePass<'a, 'p> {
    pub(super) stage: Stage<'a, DrawCtx<'a>>,
    pub(super) probe: &'a mut BreakerProbe<'p>,
}

impl NodeUi {
    /// Drop the focus hysteresis. `row_tracks` and `paint_order` are scratch grown to the
    /// largest scene seen, not gesture state, so they keep their capacity across the reset.
    pub(super) const fn reset(&mut self) {
        self.focus_kept_last = None;
    }

    /// Record the widget tree of every scene node `pass` retains (plus the focus-owning node),
    /// skipping off-screen ones entirely.
    pub(super) fn draw_all(&mut self, ui: &mut Ui, pass: &mut NodePass<'_, '_>) {
        let draw_ctx = pass.stage.ctx;
        // Paint back-to-front, so the last item drawn is frontmost. Each item's depth is
        // persisted view state; `GraphIntent::Raise` lifts a clicked item past the rest.
        //
        // Culled nodes are skipped entirely: no measure, arrange, or paint. Every widget id in a
        // node's subtree derives from its `NodeId`, so culling a sibling can't re-key anything
        // that stays on screen. Palantir drops widget state for ids not recorded this frame, so
        // a node whose subtree holds the keyboard focus (an in-progress title/const/port edit)
        // stays recorded even off-screen, or panning away mid-edit would discard the draft. The
        // exemption carries one frame past the blur (`focus_kept_last`).
        let mut focus_kept = None;
        // Swapped out for the sweep because each node hands the whole `NodeUi` to its widget.
        let mut order = mem::take(&mut self.paint_order);
        let graph_ctx = draw_ctx.graph_ctx();
        graph_ctx.paint_order(&mut order);
        for item in &order {
            let n = graph_ctx
                .node(item.node_id)
                .expect("the paint order lists the context's own placements");
            let keeps_focus = ui.is_focus_within(wid::body(n.node_id));
            if keeps_focus {
                focus_kept = Some(n.node_id);
            }
            let world = draw_ctx.geometry().node_world_rect(n);
            if !draw_ctx.cull().keeps_node(world)
                && !keeps_focus
                && self.focus_kept_last != Some(n.node_id)
            {
                // A breaker that panned or zoomed mid-stroke still cuts what it crossed out of
                // view: culled is unpainted, not unprobed.
                if world.is_some_and(|rect| pass.probe.crosses_rect(rect)) {
                    pass.probe.mark_broken_node(n.node_id);
                }
                continue;
            }
            let node_ctx = n.with_hover(wid::hover(ui, n.node_id));
            self.draw_node(ui, node_ctx, pass);
        }
        self.paint_order = order;
        self.focus_kept_last = focus_kept;
    }

    /// Record one node's body, `node_ctx`, over `row_tracks`, one buffer the whole frame slices out
    /// of, which per-node widget state would reallocate.
    fn draw_node(&mut self, ui: &mut Ui, node_ctx: NodeCtx<'_>, pass: &mut NodePass<'_, '_>) {
        let NodePass { stage, probe } = pass;
        let draw_ctx = stage.ctx;
        let (theme, node) = (node_ctx.theme(), node_ctx);

        // Probe the body against the breaker polyline: a hit recolors the border and flags the
        // node for deletion on release. The rect is the `node_world_rect` the cull and the
        // rubber band test, so all three agree on where the node is even when the document
        // moved it out from under a live gesture. A node that has never recorded has no size
        // yet, so the breaker can't catch it until next frame; the user can't aim at it anyway.
        let broken = draw_ctx
            .geometry()
            .node_world_rect(node)
            .is_some_and(|r| probe.crosses_rect(r));
        if broken {
            probe.mark_broken_node(node.node_id);
        }
        let selected = draw_ctx.is_selected(ItemId::Node(node_ctx.node_id));
        // The border width is always the selection width so selecting a node never resizes it
        // (stroke folds into padding, width-gated). Only the color changes: the breaker alarm
        // wins, then the missing-stub color, then `Theme::card_border`'s selected or resting.
        let border_width = theme.card.border_width;
        let tier = if broken {
            CardTier::Broken
        } else if selected {
            CardTier::Selected
        } else {
            CardTier::Resting
        };
        let border = if node.stub() && tier != CardTier::Broken {
            // A stub for a node whose func is gone from the library reads as broken but deletable.
            theme.status.error
        } else {
            theme.card_border(tier)
        };
        // One slot: live status outranks the ambient elevation shadow.
        let shadow = node_shadow(theme, node.exec_status());
        // A node with no rects yet is laid out unseen, so it and its wires appear together: a
        // wire reads its port's rect, which only a layout gives. As egui's `sizing_pass` measures
        // a Ui it does not show, and React Flow draws an edge only once its nodes are measured.
        // Refused: painting the node without its wires for a frame.
        let visibility = if draw_ctx.geometry().node_world_rect(node).is_some() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };

        // Recorded just before the body so the pin peeks out from behind this node's corner, with
        // the node's own cull decision and stack position.
        if node.sink() {
            header::subscription_pin(
                ui,
                node,
                Hover::from(draw_ctx.geometry().subs.is_hovered(node.node_id)),
                visibility,
            );
        }

        let row_tracks = &mut self.row_tracks;
        Panel::vstack()
            .id(wid::body(node.node_id))
            .visibility(visibility)
            .at(node.pos)
            // A preview needs room for a thumbnail.
            .min_size(if node.preview() {
                (theme.card.preview_min_width, 0.0)
            } else {
                (theme.card.min_width, 0.0)
            })
            .size((Sizing::HUG, Sizing::HUG))
            .sense(Sense::CLICK | Sense::DRAG)
            .chrome(
                theme
                    .card_chrome()
                    .with_border(Border::new(border, border_width))
                    .with_shadow(shadow),
            )
            .show(ui, |ui| {
                let mut record = NodeRecord {
                    node: node_ctx,
                    stage: stage.with(draw_ctx),
                };
                header::show(ui, &mut record);
                port_row::show(ui, &mut record, row_tracks);
                // A preview has no output, so it has no cached value for the
                // footer to report — its value takes that slot instead.
                if node.preview() {
                    preview_row::show(ui, &mut record);
                } else {
                    node_footer::show(ui, node_ctx);
                }
            });
    }
}

/// The node body's one shadow: the status frame for a last-run outcome that asks for attention,
/// in place of the resting shadow every card casts.
const fn node_shadow(theme: &Theme, status: &ExecStatus) -> Shadow {
    match theme.exec_frame(status) {
        Some(color) => theme.elevation.frame(color),
        None => theme.elevation.resting,
    }
}
