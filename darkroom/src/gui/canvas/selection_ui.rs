use scenarium::NodeId;
use std::collections::BTreeSet;

use glam::Vec2;
use palantir::{Rect, Shape, Stroke, Ui};

use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::canvas::ctx::CanvasCtx;
use crate::gui::canvas::gesture_slot::GestureSlot;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::theme::Theme;

/// Rubber-band multi-selection. A plain left-drag on empty canvas
/// sweeps a rectangle; intersecting nodes highlight live as it moves and
/// the set is committed on
/// release. Holding Shift at drag-start *extends* the current selection
/// instead of replacing it. Ctrl+LMB is the breaker and RMB opens the
/// new-node menu / breaker, so this only claims unmodified left-drags
/// that fall through to the bare canvas (node bodies hit-test first, so
/// a drag that starts on a node never reaches here).
#[derive(Default, Debug)]
pub(super) struct SelectionUI {
    band: GestureSlot<RubberBand>,
    /// The swept set while a band is active — the same type as the
    /// committed selection it stands in for (`GraphView::selected`), so the
    /// draw substitutes one for the other directly. Owned here rather than
    /// written into the document, which the gesture only touches once, on
    /// release.
    ///
    /// Refilled from scratch every frame of the drag and kept only to reuse
    /// its allocation; [`Self::preview`] is what says whether its contents
    /// mean anything.
    swept: BTreeSet<NodeId>,
    /// The pane whose draw reads [`Self::swept`]. Empty when no band is
    /// in flight — draw falls back to the committed selection. A slot of
    /// its own, not the band's, because it deliberately outlives the band
    /// by one frame: the release frame paints the final selection while
    /// the `SetSelection` is still draining.
    preview: GestureSlot<()>,
}

#[derive(Clone, Debug)]
struct RubberBand {
    /// Anchor + live corner in inner-canvas pre-transform (world)
    /// coords — the same frame node positions live in — so the rect and
    /// its hit-test need no extra transform. `current` is refreshed from
    /// the pointer every frame.
    start: Vec2,
    current: Vec2,
    /// Pre-drag selection captured at latch (empty unless Shift extends).
    /// The swept set unions onto this each frame, so we never re-read
    /// `scene.selected` mid-drag — no dependency on the document staying
    /// untouched, and the additive base is fixed at latch.
    ///
    /// Inside the band, not beside it: it is captured with the gesture
    /// and meaningless without one, and at struct level it outlived every
    /// commit as stale state.
    base: BTreeSet<NodeId>,
}

impl RubberBand {
    fn rect(&self) -> Rect {
        let min = self.start.min(self.current);
        let max = self.start.max(self.current);
        Rect::new(min.x, min.y, max.x - min.x, max.y - min.y)
    }
}

impl SelectionUI {
    /// The live swept set while a band is in flight over this pane,
    /// for node/pin draw to paint against; `None` for every other pane and
    /// when no band is active (the caller falls back to the pane's
    /// committed selection).
    pub(super) fn preview(&self) -> Option<&BTreeSet<NodeId>> {
        self.preview.get()?;
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
    /// Called once per visible graph pane; a band in flight belongs to
    /// exactly one of them, so every other pane's call returns
    /// immediately rather than advancing the band in its own coordinates.
    pub(super) fn apply(&mut self, ui: &mut Ui, cx: CanvasCtx<'_>, out: &mut Intents) {
        let graph_ctx = cx.graph_ctx();
        let resp = ui.response_for(outer_canvas_widget_id());
        if self.band.is_idle()
            && cx.gesture() == Some(CanvasGesture::Select)
            && let Some(p) = resp.pointer_local
        {
            let w = to_world(p, &graph_ctx.viewport());
            let band = RubberBand {
                start: w,
                current: w,
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
            self.band.latch(band);
        }
        if cx.cancelled() {
            self.band.clear();
        }
        let Some(mut band) = self.band.take() else {
            // No band in flight — just cancelled, or committed last frame.
            // Either way drop the preview so node draw falls back to the
            // committed selection.
            self.preview.clear();
            return;
        };
        if let Some(p) = resp.pointer_local {
            band.current = to_world(p, &graph_ctx.viewport());
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
            let Some(body) = cx.geometry().node_world_rect(n) else {
                continue;
            };
            if rect.intersects(body) {
                swept.insert(n.id);
            }
        }
        // Still dragging → stash the updated corner and leave the preview in
        // place (node draw reads it via `preview()` for live highlight).
        // A `None` delta is the release edge that commits.
        if resp.left.drag.delta().is_some() {
            self.band.latch(band);
            self.preview.latch(());
            return;
        }
        // Only the committing frame pays for the owned set the intent
        // carries. The preview stays up for *this* (release) frame's draw so
        // it paints the final selection; the `SetSelection` drains
        // post-record, and next frame — band now `None` — the early return
        // above clears the preview and draw falls back to the committed set.
        out.push(GraphIntent::SetSelection { to: swept.clone() });
        self.preview.latch(());
    }

    /// Paint the in-progress rectangle. Drawn inside the inner canvas so
    /// its world coords ride the same pan/zoom transform as the nodes.
    /// No-op when no gesture is active on this pane or the rect has
    /// no area yet.
    pub(super) fn draw(&self, ui: &mut Ui, theme: &Theme) {
        let Some(band) = self.band.get() else {
            return;
        };
        let rect = band.rect();
        if rect.area() <= f32::EPSILON {
            return;
        }
        ui.add_shape(
            Shape::rect(rect)
                .fill(theme.colors.selection_fill())
                .stroke(Stroke::solid(theme.colors.selection_border(), 1.0)),
        );
    }
}
