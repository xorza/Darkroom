use std::collections::BTreeSet;

use aperture::{PointerButton, Rect, Shape, Stroke, Ui};
use glam::Vec2;

use crate::core::document::ItemRef;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::pin_ui;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::scene::Scene;

/// Rubber-band multi-selection. A plain left-drag on empty canvas
/// sweeps a rectangle; intersecting nodes *and* pinned-output preview
/// widgets highlight live as it moves and the set is committed on
/// release. Holding Shift at drag-start *extends* the current selection
/// instead of replacing it. Cmd+LMB is the breaker and RMB opens the
/// new-node menu / breaker, so this only claims unmodified left-drags
/// that fall through to the bare canvas (node bodies hit-test first, so
/// a drag that starts on a node never reaches here).
#[derive(Default, Debug)]
pub(crate) struct SelectionUI {
    band: Option<RubberBand>,
    /// Pre-drag selection captured at latch (empty unless Shift extends).
    /// The swept set unions onto this each frame, so we never re-read
    /// `scene.selected` mid-drag — no dependency on the document staying
    /// untouched, and the additive base is fixed at latch.
    base: BTreeSet<ItemRef>,
    /// The swept set while a band is active, for node/pin draw to
    /// highlight live. Owned here rather than written into
    /// `Scene::selected` so the projection stays a read-only mirror of
    /// `Document`. `None` when no band is in flight (draw falls back to
    /// the committed selection).
    preview: Option<BTreeSet<ItemRef>>,
}

#[derive(Clone, Copy, Debug)]
struct RubberBand {
    /// Anchor + live corner in inner-canvas pre-transform (world)
    /// coords — the same frame node positions live in — so the rect and
    /// its hit-test need no extra transform. `current` is refreshed from
    /// the pointer every frame.
    start: Vec2,
    current: Vec2,
}

impl RubberBand {
    fn rect(&self) -> Rect {
        let min = self.start.min(self.current);
        let max = self.start.max(self.current);
        Rect::new(min.x, min.y, max.x - min.x, max.y - min.y)
    }
}

impl SelectionUI {
    /// The live swept set while a band is in flight, for node/pin draw to
    /// paint against; `None` when no band is active (the caller falls back
    /// to the committed `scene.selected`).
    pub(crate) fn preview(&self) -> Option<&BTreeSet<ItemRef>> {
        self.preview.as_ref()
    }

    /// Drive the gesture from the outer-canvas response: latch on an
    /// unmodified left-drag-start, track the live corner, and recompute
    /// the swept set every frame. The set is stashed in `self.preview` so
    /// nodes highlight *live* as the rectangle moves (read back via
    /// [`Self::preview`]); `Document`/undo are only touched once, by the
    /// committing `SetSelection` emitted on release. Esc cancels without
    /// emitting.
    ///
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        gesture: Option<CanvasGesture>,
        out: &mut Vec<Intent>,
    ) {
        let resp = ui.response_for(outer_canvas_widget_id());
        if self.band.is_none()
            && gesture == Some(CanvasGesture::Select)
            && let Some(p) = resp.pointer_local
        {
            let w = to_world(p, &scene.viewport);
            // Shift is a gesture *parameter* (extend vs replace), not
            // arbitration — read it here, not in the classifier. Capture
            // the base once so the per-frame union doesn't re-read the doc.
            self.base = if ui.modifiers().shift {
                scene.selected.clone()
            } else {
                BTreeSet::new()
            };
            self.band = Some(RubberBand {
                start: w,
                current: w,
            });
        }
        let Some(mut band) = self.band else {
            // No band in flight — drop any preview left by the
            // just-committed (or cancelled) drag so node draw falls back
            // to the now-committed `scene.selected`.
            self.preview = None;
            return;
        };
        if ui.escape_pressed() {
            self.band = None;
            self.preview = None;
            return;
        }
        if let Some(p) = resp.pointer_local {
            band.current = to_world(p, &scene.viewport);
        }
        let rect = band.rect();
        let mut selected: BTreeSet<ItemRef> = self.base.clone();
        for n in &scene.nodes {
            // The cached-size world rect, so nodes the viewport cull
            // skipped this frame still sweep. Never-measured nodes
            // (first frame) can't be hit yet — skip.
            let Some(body) = geometry.node_world_rect(n) else {
                continue;
            };
            if rect.intersects(body) {
                selected.insert(ItemRef::Node(n.id));
            }
        }
        for pin in scene.pinned_outputs() {
            if rect.intersects(pin_ui::pin_preview_rect(pin.pos)) {
                selected.insert(ItemRef::Pin(pin.port));
            }
        }
        // Still dragging → stash the updated corner + the swept preview
        // (node draw reads it via `preview()` for live highlight) and
        // wait. A `None` delta is the release edge that commits.
        if resp.drag_delta_by(PointerButton::Left).is_some() {
            self.band = Some(band);
            self.preview = Some(selected);
            return;
        }
        // Keep the swept set as the preview for *this* (release) frame's
        // draw so it paints the final selection; the `SetSelection` drains
        // post-record, and next frame — band now `None` — the early return
        // above clears the preview and draw falls back to the committed set.
        out.push(Intent::SetSelection {
            to: selected.clone(),
        });
        self.preview = Some(selected);
        self.band = None;
    }

    /// Paint the in-progress rectangle. Drawn inside the inner canvas so
    /// its world coords ride the same pan/zoom transform as the nodes.
    /// No-op when no gesture is active or the rect has no area yet.
    pub(crate) fn draw(&self, ui: &mut Ui, ctx: &AppContext<'_>) {
        let Some(band) = self.band else {
            return;
        };
        let rect = band.rect();
        if rect.area() <= f32::EPSILON {
            return;
        }
        let tint = ctx.theme.colors.selection_rect;
        ui.add_shape(
            Shape::rect(rect)
                .fill(tint.with_alpha(0.12))
                .stroke(Stroke::solid(tint.with_alpha(0.85), 1.0)),
        );
    }
}
