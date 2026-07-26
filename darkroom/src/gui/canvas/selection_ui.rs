use std::collections::BTreeSet;

use glam::Vec2;
use palantir::{Rect, Shape, Stroke, Ui};

use crate::core::document::{GraphRef, ItemRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::pin_ui;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::scene::GraphScene;

/// Rubber-band multi-selection. A plain left-drag on empty canvas
/// sweeps a rectangle; intersecting nodes *and* pinned-output preview
/// widgets highlight live as it moves and the set is committed on
/// release. Holding Shift at drag-start *extends* the current selection
/// instead of replacing it. Cmd+LMB is the breaker and RMB opens the
/// new-node menu / breaker, so this only claims unmodified left-drags
/// that fall through to the bare canvas (node bodies hit-test first, so
/// a drag that starts on a node never reaches here).
#[derive(Default, Debug)]
pub(super) struct SelectionUI {
    band: Option<RubberBand>,
    /// Pre-drag selection captured at latch (empty unless Shift extends).
    /// The swept set unions onto this each frame, so we never re-read
    /// `scene.selected` mid-drag — no dependency on the document staying
    /// untouched, and the additive base is fixed at latch.
    base: BTreeSet<ItemRef>,
    /// The swept set while a band is active, with the pane it belongs to,
    /// for that pane's node/pin draw to highlight live — sorted, like the
    /// committed spans it stands in for. Owned here rather than written
    /// into the projection so that stays a read-only mirror of
    /// `Document`. `None` when no band is in flight (draw falls back to
    /// the committed selection).
    preview: Option<(GraphRef, Vec<ItemRef>)>,
}

#[derive(Clone, Copy, Debug)]
struct RubberBand {
    /// The pane the band was latched on. Every visible graph pane runs
    /// this controller, so without it a band started on one canvas would
    /// be advanced (and drawn) by its neighbours, in their coordinates.
    graph: GraphRef,
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
    /// The live swept set while a band is in flight over `graph`'s pane,
    /// for node/pin draw to paint against; `None` for every other pane and
    /// when no band is active (the caller falls back to the pane's
    /// committed selection).
    pub(super) fn preview(&self, graph: GraphRef) -> Option<&[ItemRef]> {
        let (owner, swept) = self.preview.as_ref()?;
        (*owner == graph).then_some(swept.as_slice())
    }

    /// Drive the gesture from the outer-canvas response: latch on an
    /// unmodified left-drag-start, track the live corner, and recompute
    /// the swept set every frame. The set is stashed in `self.preview` so
    /// nodes highlight *live* as the rectangle moves (read back via
    /// [`Self::preview`]); `Document`/undo are only touched once, by the
    /// committing `SetSelection` emitted on release. Esc cancels without
    /// emitting.
    ///
    /// Called once per visible graph pane; a band in flight belongs to
    /// exactly one of them, so every other pane's call returns
    /// immediately rather than advancing the band in its own coordinates.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        graph: GraphScene<'_>,
        geometry: &CanvasGeometry,
        gesture: Option<CanvasGesture>,
        out: &mut Intents,
    ) {
        let target = graph.target();
        if self.band.is_some_and(|band| band.graph != target) {
            return;
        }
        let resp = ui.response_for(outer_canvas_widget_id(target));
        if self.band.is_none()
            && gesture == Some(CanvasGesture::Select)
            && let Some(p) = resp.pointer_local
        {
            let w = to_world(p, &graph.viewport());
            // Shift is a gesture *parameter* (extend vs replace), not
            // arbitration — read it here, not in the classifier. Capture
            // the base once so the per-frame union doesn't re-read the doc.
            self.base = if ui.modifiers().shift {
                graph.selection()
            } else {
                BTreeSet::new()
            };
            self.band = Some(RubberBand {
                graph: target,
                start: w,
                current: w,
            });
        }
        let Some(mut band) = self.band else {
            // No band in flight — drop any preview left by the
            // just-committed (or cancelled) drag so node draw falls back
            // to the now-committed selection.
            self.preview = None;
            return;
        };
        if ui.escape_pressed() {
            self.band = None;
            self.preview = None;
            return;
        }
        if let Some(p) = resp.pointer_local {
            band.current = to_world(p, &graph.viewport());
        }
        let rect = band.rect();
        let mut selected: BTreeSet<ItemRef> = self.base.clone();
        for n in graph.nodes() {
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
        for pin in graph.pinned_outputs() {
            if rect.intersects(pin_ui::pin_preview_rect(pin.pos)) {
                selected.insert(ItemRef::Pin(pin.port));
            }
        }
        // `BTreeSet` iterates sorted, which is what the slice readers
        // (`RecordCtx::is_selected`) binary-search.
        let swept: Vec<ItemRef> = selected.iter().copied().collect();
        // Still dragging → stash the updated corner + the swept preview
        // (node draw reads it via `preview()` for live highlight) and
        // wait. A `None` delta is the release edge that commits.
        if resp.left.drag.delta().is_some() {
            self.band = Some(band);
            self.preview = Some((target, swept));
            return;
        }
        // Keep the swept set as the preview for *this* (release) frame's
        // draw so it paints the final selection; the `SetSelection` drains
        // post-record, and next frame — band now `None` — the early return
        // above clears the preview and draw falls back to the committed set.
        out.for_graph(target, |out| {
            out.push(Intent::SetSelection { to: selected })
        });
        self.preview = Some((target, swept));
        self.band = None;
    }

    /// Paint the in-progress rectangle. Drawn inside the inner canvas so
    /// its world coords ride the same pan/zoom transform as the nodes.
    /// No-op when no gesture is active on `graph`'s pane or the rect has
    /// no area yet.
    pub(super) fn draw(&self, ui: &mut Ui, ctx: &AppContext<'_>, graph: GraphRef) {
        let Some(band) = self.band.filter(|band| band.graph == graph) else {
            return;
        };
        let rect = band.rect();
        if rect.area() <= f32::EPSILON {
            return;
        }
        ui.add_shape(
            Shape::rect(rect)
                .fill(ctx.theme.colors.selection_fill())
                .stroke(Stroke::solid(ctx.theme.colors.selection_border(), 1.0)),
        );
    }
}
