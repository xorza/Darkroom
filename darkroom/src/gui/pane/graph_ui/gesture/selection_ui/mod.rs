use std::collections::BTreeSet;

use glam::Vec2;
use palantir::widget::Shape;
use palantir::{Rect, Stroke, Ui};

use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::prepass_ctx::CanvasGesture;
use crate::gui::theme::Theme;

/// Rubber-band multi-selection. A plain left-drag on empty canvas
/// sweeps a rectangle; the nodes it intersects and the ties whose centers it
/// holds highlight live as it moves, and the set is committed on release. Holding Shift at drag-start *extends* the current selection
/// instead of replacing it. Ctrl+LMB is the breaker and RMB opens the
/// new-node menu / breaker, so this only claims unmodified left-drags
/// that fall through to the bare canvas (node bodies hit-test first, so
/// a drag that starts on a node never reaches here).
#[derive(Default, Debug)]
pub(crate) struct SelectionUi {
    band: Option<RubberBand>,
    /// The swept set while a band is active, **sorted and deduplicated** so
    /// [`Selection::contains`](crate::gui::pane::graph_ui::selection::Selection::contains) can
    /// binary-search it. Owned here rather than written into the document, which the gesture only
    /// touches once, on release.
    ///
    /// Refilled from scratch every frame of the drag, which is what makes the
    /// container choice load-bearing: a `Vec`'s `clear` keeps the allocation
    /// for the next frame, where a `BTreeSet` would drop its whole tree and
    /// rebuild it every frame of every drag.
    /// [`Self::preview`] is what says whether the contents mean anything.
    swept: Vec<ItemId>,
    /// Whether the draw reads [`Self::swept`]. Empty when no band is in
    /// flight — draw falls back to the committed selection. A slot of
    /// its own, not the band's, because it deliberately outlives the band
    /// by one frame: the release frame paints the final selection while
    /// the `SetSelection` is still draining.
    preview: bool,
}

#[derive(Clone, Debug)]
struct RubberBand {
    /// Anchor + live corner in inner-canvas pre-transform (world)
    /// coords — the same frame node positions live in — so the rect and
    /// its hit-test need no extra transform. `cur` is refreshed from
    /// the pointer every frame.
    start: Vec2,
    cur: Vec2,
    /// Pre-drag selection captured at latch (empty unless Shift extends).
    /// The swept set unions onto this each frame, so we never re-read
    /// `GraphView::selected` mid-drag — no dependency on the document staying
    /// untouched, and the additive base is fixed at latch.
    ///
    /// Inside the band, not beside it: it is captured with the gesture
    /// and meaningless without one, and at struct level it outlived every
    /// commit as stale state.
    base: BTreeSet<ItemId>,
}

impl RubberBand {
    fn rect(&self) -> Rect {
        Rect::from_min_max(self.start.min(self.cur), self.start.max(self.cur))
    }
}

impl SelectionUi {
    /// Drop the in-flight band and its preview. `swept` is cleared in place —
    /// keeping the allocation is the whole reason it is a `Vec`.
    pub(crate) fn reset(&mut self) {
        self.band = None;
        self.swept.clear();
        self.preview = false;
    }

    /// Whether a band is in flight.
    pub(crate) const fn in_flight(&self) -> bool {
        self.band.is_some()
    }

    /// The live swept set while a band is in flight, for node/pin draw to
    /// paint against; `None` when no band is active (the caller falls back to
    /// the committed selection).
    pub(crate) fn preview(&self) -> Option<&[ItemId]> {
        if !self.preview {
            return None;
        }
        Some(&self.swept)
    }

    /// Drive the gesture from the outer-canvas response: latch on an
    /// unmodified left-drag-start, track the live corner, and recompute
    /// the swept set every frame. The set is stashed in `self.preview` so
    /// nodes highlight *live* as the rectangle moves (read back via
    /// [`Self::preview`]); `Document`/undo are only touched once, by the
    /// committing `SetSelection` emitted on release. The context's Esc —
    /// resolved once by the canvas — drops the band without emitting.
    ///
    /// A click on bare canvas clears the selection: node panels hit-test first, so it fires only
    /// when the click missed every node, and an empty selection records no no-op entry. A *drag*
    /// there is the rubber band.
    pub(crate) fn apply(&mut self, ui: &mut Ui, ctx: CanvasCtx<'_>, out: &mut Requests) {
        let graph_ctx = ctx.graph_ctx();
        if ctx.gesture() == Some(CanvasGesture::Deselect) && !graph_ctx.selected().is_empty() {
            out.push_graph(GraphIntent::clear_selection());
        }
        let resp = ui.response_for(canvas::outer_wid());
        if self.band.is_none()
            && ctx.gesture() == Some(CanvasGesture::Select)
            && let Some(p) = resp.pointer_local
        {
            let w = graph_ctx.viewport().to_world(p);
            let band = RubberBand {
                start: w,
                cur: w,
                // Shift is a gesture *parameter* (extend vs replace), not
                // arbitration — read it here, not in the classifier.
                // Captured once, so the per-frame union never re-reads the
                // document.
                base: if ui.modifiers().shift {
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
            // No band in flight — just cancelled, or committed last frame.
            // Either way drop the preview so node draw falls back to the
            // committed selection.
            self.preview = false;
            return;
        };
        if let Some(p) = resp.pointer_local {
            band.cur = graph_ctx.viewport().to_world(p);
        }
        let rect = band.rect();
        // Swept into the reused preview buffer: refilled from scratch every
        // frame the band is held, keeping its allocation across frames.
        let swept = &mut self.swept;
        swept.clear();
        swept.extend(band.base.iter().copied());
        for n in graph_ctx.nodes() {
            // The cached-size world rect, so nodes the viewport cull
            // skipped this frame still sweep. Never-measured nodes
            // (first frame) can't be hit yet — skip.
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
        // The base arrives sorted (it came off a `BTreeSet`) but the sweep
        // appends in paint order and can re-add a node the base already held,
        // so both properties `Selection::Swept` promises are restored here.
        swept.sort_unstable();
        swept.dedup();
        // Still dragging → stash the updated corner and leave the preview in
        // place (node draw reads it via `preview()` for live highlight).
        // A `None` delta is the release edge that commits.
        if resp.left.drag.delta().is_some() {
            self.band = Some(band);
            self.preview = true;
            return;
        }
        // Only the committing frame pays for the owned set the intent
        // carries. The preview stays up for *this* (release) frame's draw so
        // it paints the final selection; the `SetSelection` drains
        // post-record, and next frame — band now `None` — the early return
        // above clears the preview and draw falls back to the committed set.
        out.push_graph(GraphIntent::SetSelection {
            to: swept.iter().copied().collect(),
        });
        self.preview = true;
    }

    /// Paint the in-progress rectangle. Drawn inside the inner canvas so
    /// its world coords ride the same pan/zoom transform as the nodes.
    /// No-op when no gesture is active or the rect has no area yet.
    ///
    /// The border is a hairline on screen at every `zoom`: palantir scales a stroke by the
    /// canvas's transform, so its world width is the hairline over the zoom.
    pub(crate) fn draw(&self, ui: &mut Ui, theme: &Theme, zoom: f32) {
        let Some(band) = self.band.as_ref() else {
            return;
        };
        let rect = band.rect();
        if rect.area() <= f32::EPSILON {
            return;
        }
        ui.add_shape(
            Shape::rect(rect)
                .fill(theme.colors.selection_fill())
                .border(border(theme, zoom)),
        );
    }
}

/// The band's border at `zoom`, a hairline on screen.
fn border(theme: &Theme, zoom: f32) -> Stroke {
    Stroke::new(
        theme.colors.selection_border(),
        theme.stroke.hairline / zoom,
    )
}

#[cfg(test)]
mod tests;
