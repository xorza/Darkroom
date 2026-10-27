mod card;
mod const_editor;
pub(super) mod header;
mod node_footer;
pub(super) mod node_record;
pub(crate) mod port_row;
pub(crate) mod preview_row;

use std::mem;

use palantir::prelude::*;
use scenarium::NodeId;

use crate::core::document::graph_view::StackedNode;
use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::gesture::breaker_ui::breaker_probe::BreakerProbe;
use crate::gui::pane::graph_ui::gesture::group_drag::{self, Grab};
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::wid;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::{CardTier, Theme};
use crate::gui::widgets::hover::Hover;

/// Owns rendering of every graph node; a body or title press reports its grab, which the
/// canvas's one group drag latches.
///
/// `draw_all` is the single entry point; `GraphUi` calls it once per frame after
/// [`crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry`] has been rebuilt from
/// last-frame's responses.
#[derive(Default, Debug)]
pub(super) struct NodeUi {
    /// The node kept recorded by the focus cull-exemption last frame.
    /// Focus clears during input, *before* the record, so on the blur
    /// frame `is_focus_within` is already false — but that frame is exactly
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
    paint_order: Vec<StackedNode>,
}

/// The canvas pass every node body records in: the draw context, the breaker
/// probe the bodies mark hits against, and the queue their widgets raise edits
/// into.
#[derive(Debug)]
pub(super) struct NodePass<'a, 'p> {
    pub(super) draw_ctx: DrawCtx<'a>,
    pub(super) probe: &'a mut BreakerProbe<'p>,
    pub(super) out: &'a mut Requests,
}

/// What one record pass's node draw saw but could not act on itself.
///
/// Each drives state the draw itself holds *shared* — the context menu because
/// it is the canvas's and not a node's, the group drag because the canvas owns
/// one — so the clicks are seen here and applied by the caller once the draw is
/// over and both can be taken `&mut`.
#[derive(Default, Debug)]
pub(crate) struct NodeDrawOutcome {
    /// The node whose body was right-clicked, opening its context menu.
    pub(crate) menu_opened: Option<NodeId>,
    /// The drag a node's body or title press started, for the canvas's one group drag to latch.
    pub(crate) grab: Option<Grab>,
}

/// What drawing **one** node reports back: the signals its own widgets
/// produced but could not act on, in the shape palantir's
/// `TextEditResponse` uses — the drawer is authoritative about what it did
/// with this frame's input, so no caller re-polls for it.
///
/// Booleans, because this is one node. Which node across the scene is
/// [`NodeDrawOutcome`]'s
/// question, and folding these into it is the loop's
/// job rather than the node's.
#[derive(Default, Debug)]
pub(super) struct NodeResponse {
    pub(super) menu_opened: bool,
    /// The drag a body or title press started, for the canvas's one group drag to latch.
    pub(super) grab: Option<Grab>,
}

impl NodeUi {
    /// Drop the focus hysteresis. `row_tracks` and `paint_order` are scratch grown to the
    /// largest scene seen, not gesture state, so they keep their capacity across the reset.
    pub(super) const fn reset(&mut self) {
        self.focus_kept_last = None;
    }

    /// Record the widget tree of every scene node `pass` retains (plus
    /// the focus-owning node — see the loop comment), skipping off-screen ones
    /// entirely, and report what the draw saw but could not act on — see
    /// [`NodeDrawOutcome`]. Emits selection/raise intents for body clicks and
    /// reports the grab of a body/title drag (port circles capture
    /// their own presses via `Sense::CLICK`, so drags don't latch off the port
    /// grabs).
    pub(super) fn draw_all(&mut self, ui: &mut Ui, pass: &mut NodePass<'_, '_>) -> NodeDrawOutcome {
        let draw_ctx = pass.draw_ctx;
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
        // keyboard focus (`is_focus_within` — an in-progress title/const/port
        // edit) stays recorded even off-screen; otherwise panning away
        // mid-edit would discard the draft. The exemption carries one frame
        // past the blur (`focus_kept_last`): focus clears before the record,
        // and that first post-blur record is where the edit's pending draft
        // commits.
        let mut outcome = NodeDrawOutcome::default();
        let mut focus_kept = None;
        // Swapped out for the sweep because each node hands the whole `NodeUi`
        // to its widget; it goes back below with its capacity, so only a graph
        // larger than every earlier one ever allocates here.
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
            let node = self.draw_node(ui, node_ctx, pass);
            if node.menu_opened {
                outcome.menu_opened.get_or_insert(n.node_id);
            }
            if outcome.grab.is_none() {
                outcome.grab = node.grab;
            }
        }
        self.paint_order = order;
        self.focus_kept_last = focus_kept;
        outcome
    }

    /// Record one node's body, `node_ctx`, over the state every node on the canvas shares: one
    /// pointer so one drag, and `row_tracks` one buffer the whole frame slices out of, which
    /// per-node widget state would reallocate.
    fn draw_node(
        &mut self,
        ui: &mut Ui,
        node_ctx: NodeCtx<'_>,
        pass: &mut NodePass<'_, '_>,
    ) -> NodeResponse {
        let state = self;
        let NodePass {
            draw_ctx,
            probe,
            out,
        } = pass;
        let draw_ctx = *draw_ctx;
        let (theme, node) = (node_ctx.theme(), node_ctx);

        // Probe the body against the breaker polyline. Hit → recolor border
        // red and flag the node for deletion on release. The rect is the same
        // `node_world_rect` the cull and the rubber band test — this frame's
        // position plus the cached measured size — so all three agree
        // on where the node is even when the document moved it out from under
        // a live gesture (an undo, say). A node that has never
        // recorded has no size yet, so the breaker can't catch it until next
        // frame: acceptable, since the user can't aim at something unpainted.
        let broken = draw_ctx
            .geometry()
            .node_world_rect(node)
            .is_some_and(|r| probe.crosses_rect(r));
        if broken {
            probe.mark_broken_node(node.node_id);
        }
        let selected = draw_ctx.is_selected(ItemId::Node(node_ctx.node_id));
        // The border width is *always* the selection width so selecting a
        // node never resizes it (stroke folds into padding — width-gated,
        // not color-gated). Only the color changes: the breaker alarm wins,
        // then the missing-stub color, then `Theme::card_border`'s selected
        // or resting color.
        let border_width = theme.card.border_width_total();
        let tier = if broken {
            CardTier::Broken
        } else if selected {
            CardTier::Selected
        } else {
            CardTier::Resting
        };
        let border = if node.stub() && tier != CardTier::Broken {
            // A stub for a node whose func is gone from the library: paint it
            // in the error color so it reads as broken-but-deletable.
            theme.status.error
        } else {
            theme.card_border(tier)
        };
        // Sample modifiers before the panel borrows `ui` for the rest
        // of this scope (the click handler below can't reborrow it).
        let click_mode = ClickMode::with_shift(ui.modifiers().shift);
        // The status frame when the last run asks for attention, else the
        // ambient elevation shadow — one slot, and live status outranks depth.
        let shadow = node_shadow(theme, node.exec_status());

        // The subscription pin records just before the body so it peeks out
        // from behind this node's corner while riding the same cull decision
        // and stack position as the node itself.
        if node.sink() {
            header::subscription_pin(
                ui,
                node,
                Hover::from(draw_ctx.geometry().subs.is_hovered(node.node_id)),
            );
        }

        // Borrowed off `self` before the body closure so it can't conflict
        // with the drag latch below, which reads a different field.
        let row_tracks = &mut state.row_tracks;
        let panel = Panel::vstack()
            .id(wid::body(node.node_id))
            .pos(node.pos)
            // A preview needs room for a thumbnail; every other node keeps the
            // theme's own floor.
            .min_size(if node.preview() {
                (preview_row::PREVIEW_MIN_WIDTH, theme.card.min_height)
            } else {
                (theme.card.min_width, theme.card.min_height)
            })
            .size((Sizing::HUG, Sizing::HUG))
            .sense(Sense::CLICK | Sense::DRAG)
            .chrome(
                Chrome::rounded(theme.card.fill, Corners::all(theme.card.corner_radius))
                    .with_border(Stroke::new(border, border_width))
                    .with_shadow(shadow),
            )
            .show(ui, |ui| {
                let mut record = NodeRecord {
                    node: node_ctx,
                    draw: draw_ctx,
                    out,
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
        // Pull the body response's flags into locals so its `&Ui` borrow ends
        // before the handle scan below. (`Response` is a lazy probe over
        // `response_for`, so reading the body through either is the same
        // last-frame state.)
        let body_clicked = panel.response.left.clicked();
        let mut response = NodeResponse {
            menu_opened: panel.response.right.clicked(),
            grab: None,
        };

        // Click without drag → select. Plain click selects only this
        // node; Shift-click toggles its membership in the current
        // selection. `UndoStep::is_noop` filters a click that doesn't
        // change the set (e.g. clicking the sole selected node).
        if body_clicked {
            out.extend_graph(GraphIntent::click(
                click_mode,
                node_ctx.graph_ctx.selected(),
                ItemId::Node(node.node_id),
            ));
        }

        // Latch the anchor on the press-frame edge, off whichever of this
        // node's handles caught the press. Read here, where the handles are
        // built, over the same curated list they are drawn from; subsequent
        // frames' `prepass` peeks `response_for(widget_id)` before record runs
        // and converts `drag_delta` into a `MoveSelection` applied to
        // `Document` before the record reads it back.
        if let Some(handle) =
            wid::drag_handles(node.node_id).find(|w| ui.response_for(*w).left.drag.started())
        {
            // Grabbing a node already in the selection drags the whole
            // group together;
            // grabbing an unselected node selects only it and drags it
            // alone.
            let members = if selected {
                group_drag::selected_group(draw_ctx)
            } else {
                out.extend_graph(GraphIntent::click(
                    ClickMode::Replace,
                    node_ctx.graph_ctx.selected(),
                    ItemId::Node(node.node_id),
                ));
                group_drag::with_inner_ties(
                    node_ctx.graph_ctx.view(),
                    vec![DragStart {
                        item: ItemId::Node(node.node_id),
                        pos: node.pos,
                    }],
                )
            };
            response.grab = Some(Grab {
                item: ItemId::Node(node.node_id),
                members,
                handle,
            });
        }
        response
    }
}

/// The node body's one shadow: the status frame for a last-run outcome that
/// asks for attention (zero offset so the halo wraps evenly), or — else — a soft
/// ambient drop shadow that lifts the body off the canvas and the wires
/// crossing beneath it. The ambient color is the theme's elevation swatch
/// (`node_ambient_shadow`), shared with the other cards so all elevated
/// surfaces cast one kind of shadow.
const fn node_shadow(theme: &Theme, status: &ExecStatus) -> Shadow {
    match theme.exec_frame(status) {
        // Blur/spread sized so the frame carries elevation too — it replaces
        // the ambient shadow, and a tighter halo would leave a framed node
        // sitting flatter than its neighbors. Kept a touch tighter than the
        // ambient shadow so the status reads as a crisp halo, not a bloom.
        Some(color) => Shadow::drop(color, Vec2::ZERO, 3.0).with_spread(0.5),
        None => theme.card.elevation_shadow(10.0),
    }
}
