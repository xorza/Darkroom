use std::collections::BTreeSet;

use glam::Vec2;
use palantir::widget::Shape;
use palantir::{Border, Modifiers, Paint, Rect, Ui};

use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::stage::Stage;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::canvas_input::CanvasGesture;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::theme::Theme;

/// Rubber-band multi-selection. A plain left-drag on empty canvas sweeps a rectangle; the nodes
/// it intersects and the ties whose centers it holds highlight live as it moves, and the set is
/// committed on release. Holding Shift at drag-start extends the current selection instead of
/// replacing it. Ctrl+LMB is the breaker and RMB opens the new-node menu or the breaker, so
/// this only claims unmodified left-drags that reach the bare canvas (node bodies hit-test
/// first).
#[derive(Default, Debug)]
pub(crate) struct SelectionUi {
    band: Option<RubberBand>,
    /// The swept set while a band is active, sorted and deduplicated so
    /// [`Selection::contains`](crate::gui::pane::graph_ui::selection::Selection::contains) can
    /// binary-search it. Owned here rather than written into the document, which the gesture only
    /// touches once, on release.
    ///
    /// Refilled from scratch every frame of the drag, so a `Vec`'s `clear` keeps the allocation
    /// where a `BTreeSet` would drop and rebuild its whole tree. [`Self::preview`] says whether
    /// the contents mean anything.
    swept: Vec<ItemId>,
}

#[derive(Clone, Debug)]
struct RubberBand {
    /// Anchor and live corner in inner-canvas pre-transform (world) coords, the frame node
    /// positions live in, so the rect and its hit-test need no extra transform. `cur` is
    /// refreshed from the pointer every frame.
    start: Vec2,
    cur: Vec2,
    /// Pre-drag selection captured at latch (empty unless Shift extends). The swept set unions
    /// onto this each frame, so `GraphView::selected` is never re-read mid-drag and the additive
    /// base is fixed at latch. Inside the band, as it is meaningless without one.
    base: BTreeSet<ItemId>,
}

impl RubberBand {
    fn rect(&self) -> Rect {
        Rect::from_min_max(self.start.min(self.cur), self.start.max(self.cur))
    }
}

impl SelectionUi {
    /// Drop the in-flight band. `swept` is cleared in place, keeping its allocation.
    pub(crate) fn reset(&mut self) {
        self.band = None;
        self.swept.clear();
    }

    /// Whether a band is in flight.
    pub(crate) const fn in_flight(&self) -> bool {
        self.band.is_some()
    }

    /// The live swept set while a band is in flight, for node and pin draw to paint against;
    /// `None` when no band is active (the caller falls back to the committed selection).
    pub(crate) fn preview(&self) -> Option<&[ItemId]> {
        self.band.is_some().then_some(&self.swept)
    }

    /// Drive the gesture from the outer-canvas response: latch on an unmodified left-drag-start,
    /// track the live corner, and recompute the swept set every frame, so nodes highlight live
    /// as the rectangle moves (read back via [`Self::preview`]); `Document`/undo are only
    /// touched once, by the committing `SetSelection` emitted on release. The context's Esc
    /// drops the band without emitting.
    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, CanvasCtx<'_>>) {
        let ctx = stage.ctx;
        let graph_ctx = ctx.graph_ctx();
        let resp = ui.response_for(canvas::outer_wid());
        if self.band.is_none()
            && ctx.gesture() == Some(CanvasGesture::Select)
            && let Some(p) = resp.pointer_local
            && let Some(travel) = resp.left.drag.delta()
        {
            // The band opens where the press went down, not where the drag latched past its
            // threshold, so the corner under the press is the band's.
            let viewport = graph_ctx.camera();
            let band = RubberBand {
                start: viewport.to_world(p - travel),
                cur: viewport.to_world(p),
                // Shift is a gesture parameter (extend vs replace), not arbitration, so it is read
                // here, not in the classifier. Captured once, so the per-frame union never
                // re-reads the document.
                base: if ui.modifiers().contains(Modifiers::SHIFT) {
                    graph_ctx.selected().clone()
                } else {
                    BTreeSet::new()
                },
            };
            self.band = Some(band);
        }
        if ctx.cancelled() {
            self.band = None;
        }
        let Some(mut band) = self.band.take() else {
            return;
        };
        if let Some(p) = resp.pointer_local {
            band.cur = graph_ctx.camera().to_world(p);
        }
        let rect = band.rect();
        // Swept into the reused preview buffer, refilled every frame the band is held.
        let swept = &mut self.swept;
        swept.clear();
        swept.extend(band.base.iter().copied());
        for n in graph_ctx.nodes() {
            // The cached-size world rect, so nodes the viewport cull skipped this frame still
            // sweep. Never-measured nodes (first frame) can't be hit yet.
            let Some(body) = ctx.geometry().node_world_rect(n) else {
                continue;
            };
            if rect.intersects(body) {
                swept.push(ItemId::Node(n.node_id));
            }
        }
        // A tie is swept when the band holds its center.
        swept.extend(
            graph_ctx
                .view()
                .ties
                .iter()
                .filter(|(_, placed)| rect.contains(placed.pos))
                .map(|(&tie, _)| ItemId::Tie(tie)),
        );
        // The sweep appends in paint order and can re-add a node the base already held, so both
        // properties `Selection::Swept` promises are restored here.
        swept.sort_unstable();
        swept.dedup();
        // Still dragging → stash the updated corner. A `None` delta is the release edge that
        // commits, and only the committing frame pays for the owned set the intent carries.
        if resp.left.drag.delta().is_some() {
            self.band = Some(band);
            return;
        }
        stage.out.push_graph(GraphIntent::SetSelection {
            to: swept.iter().copied().collect(),
        });
    }

    /// Paint the in-progress rectangle, inside the inner canvas so its world coords ride the
    /// same pan/zoom transform as the nodes. No-op when no gesture is active or the rect has no
    /// area yet.
    ///
    /// The border is a hairline on screen at every `zoom`: palantir scales a stroke by the
    /// canvas's transform, so its world width is the hairline over the zoom.
    pub(crate) fn record(&self, ui: &mut Ui, ctx: DrawCtx<'_>) {
        let (theme, zoom) = (ctx.theme(), ctx.zoom());
        let Some(band) = self.band.as_ref() else {
            return;
        };
        let rect = band.rect();
        if rect.area() <= f32::EPSILON {
            return;
        }
        ui.add_shape(
            Shape::rect().at(rect),
            Paint::from(theme.colors.selection_fill()).with_border(border(theme, zoom)),
        );
    }
}

/// The band's border at `zoom`, a hairline on screen.
fn border(theme: &Theme, zoom: f32) -> Border {
    Border::new(
        theme.colors.selection_border(),
        theme.stroke.hairline / zoom,
    )
}

#[cfg(test)]
mod tests;
