use std::collections::BTreeSet;

use glam::Vec2;
use palantir::{Corners, PointerButton, Rect, Shape, Stroke, Ui};
use scenarium::prelude::NodeId;

use crate::app::AppContext;
use crate::edit::intent::Intent;
use crate::gui::canvas::{outer_canvas_widget_id, to_world};
use crate::gui::node::node_widget_id;
use crate::scene::Scene;

/// Rubber-band multi-selection. A plain left-drag on empty canvas
/// sweeps a rectangle; intersecting nodes highlight live as it moves
/// and the set is committed on release. Holding Shift at drag-start
/// *extends* the current selection instead of replacing it. Cmd+LMB is the breaker
/// and RMB opens the new-node menu / breaker, so this only claims
/// unmodified left-drags that fall through to the bare canvas (node
/// bodies hit-test first, so a drag that starts on a node never reaches
/// here).
#[derive(Default, Debug)]
pub(crate) struct SelectionUI {
    band: Option<RubberBand>,
}

#[derive(Clone, Copy, Debug)]
struct RubberBand {
    /// Anchor + live corner in inner-canvas pre-transform (world)
    /// coords — the same frame node positions live in — so the rect and
    /// its hit-test need no extra transform. `current` is refreshed from
    /// the pointer every frame.
    start: Vec2,
    current: Vec2,
    /// Shift held when the drag latched → union with the existing
    /// selection rather than replacing it.
    additive: bool,
}

impl RubberBand {
    fn rect(&self) -> Rect {
        let min = self.start.min(self.current);
        let max = self.start.max(self.current);
        Rect::new(min.x, min.y, max.x - min.x, max.y - min.y)
    }
}

impl SelectionUI {
    /// Drive the gesture from the outer-canvas response: latch on an
    /// unmodified left-drag-start, track the live corner, and recompute
    /// the swept set every frame. The set is written straight into
    /// `scene.selected_nodes` so nodes highlight *live* as the rectangle
    /// moves; `Document`/undo are only touched once, by the committing
    /// `SetSelection` emitted on release. Esc cancels without emitting.
    ///
    /// The pre-drag selection (the additive base) needs no stored copy:
    /// `Scene::rebuild` reseeds `selected_nodes` from `Document` at the
    /// top of every frame, and the document stays untouched until
    /// release, so `scene.selected_nodes` here is always the base.
    pub(crate) fn apply(&mut self, ui: &mut Ui, scene: &mut Scene, out: &mut Vec<Intent>) {
        let resp = ui.response_for(outer_canvas_widget_id());
        let mods = ui.modifiers();
        if self.band.is_none()
            && resp.drag_started_by(PointerButton::Left)
            && !mods.ctrl
            && let Some(p) = resp.pointer_local
        {
            let w = to_world(p, scene);
            self.band = Some(RubberBand {
                start: w,
                current: w,
                additive: mods.shift,
            });
        }
        let Some(mut band) = self.band else {
            return;
        };
        if ui.escape_pressed() {
            self.band = None;
            return;
        }
        if let Some(p) = resp.pointer_local {
            band.current = to_world(p, scene);
        }
        let rect = band.rect();
        let mut selected: BTreeSet<NodeId> = if band.additive {
            scene.selected_nodes.clone()
        } else {
            BTreeSet::new()
        };
        for n in &scene.nodes {
            // World body rect: `n.pos` is the node's pre-transform min
            // (fed to `Panel::position`), `layout_rect.size` is already
            // in world units. Unmeasured nodes (first frame) can't be
            // hit yet — skip.
            let Some(size) = ui
                .response_for(node_widget_id(n.id))
                .layout_rect
                .map(|r| r.size)
            else {
                continue;
            };
            if rect.intersects(Rect { min: n.pos, size }) {
                selected.insert(n.id);
            }
        }
        // Live preview: this frame's nodes draw against the swept set.
        scene.selected_nodes = selected.clone();
        // Still dragging → stash the updated corner and wait. `None`
        // delta is the release edge that commits the selection.
        if resp.drag_delta_by(PointerButton::Left).is_some() {
            self.band = Some(band);
            return;
        }
        out.push(Intent::SetSelection { to: selected });
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
        let tint = ctx.theme.selection_rect;
        ui.add_shape(Shape::RoundedRect {
            local_rect: Some(rect),
            corners: Corners::all(0.0),
            fill: tint.with_alpha(0.12).into(),
            stroke: Stroke::solid(tint.with_alpha(0.85), 1.0),
        });
    }
}
