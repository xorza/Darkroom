//! Pan and zoom handling for the graph view.
//!
//! Everything in here is a pure projection of `(state, input)` → new
//! target `(scale, pan)`, plus the state-machine transitions for the
//! middle-button pan mode. Mutation of `view_graph.pan` / `scale` happens
//! exclusively through `ZoomPanChanged::apply` in `Session::commit_actions`.

use common::BoolExt;
use egui::{PointerButton, Pos2, Response, Vec2};

use crate::common::UiEquals;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::gui::graph_ui::gesture::Gesture;
use crate::gui::graph_ui::layout::{self, GraphLayout};
use crate::gui::graph_ui::{GraphUi, MAX_ZOOM, MIN_ZOOM, WHEEL_ZOOM_SPEED};
use crate::gui::{Gui, ViewParams};
use crate::input::InputSnapshot;
use crate::model::graph_ui_action::GraphUiAction;

impl GraphUi {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_zoom_and_pan(
        &mut self,
        gui: &mut Gui<'_>,
        input: &InputSnapshot,
        ctx: &GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Option<Pos2>,
        output: &mut FrameOutput,
    ) {
        self.drive_pan_interaction_state(background_response);

        let mut new_scale = ctx.view_graph.scale;
        let mut new_pan = ctx.view_graph.pan;

        if let Some(pointer_pos) = pointer_pos {
            let vp = gui.view_params();
            (new_scale, new_pan) = compute_scroll_zoom(&vp, input, pointer_pos, new_scale, new_pan);
        }

        if matches!(self.gesture, Gesture::Panning)
            && background_response.dragged_by(PointerButton::Middle)
        {
            new_pan += background_response.drag_delta();
        }

        self.emit_zoom_pan(ctx.view_graph, new_pan, new_scale, output);
    }

    fn drive_pan_interaction_state(&mut self, background_response: &Response) {
        match &self.gesture {
            Gesture::Idle if background_response.drag_started_by(PointerButton::Middle) => {
                self.gesture.start_panning();
            }
            Gesture::Panning if background_response.drag_stopped_by(PointerButton::Middle) => {
                self.gesture.cancel();
            }
            _ => {}
        }
    }

    /// Emit `ZoomPanChanged` iff the target differs from the current view.
    /// `apply()` in `commit_actions` is the single site that writes
    /// `pan` / `scale` onto `ViewGraph`.
    pub(super) fn emit_zoom_pan(
        &mut self,
        view_graph: &crate::model::ViewGraph,
        new_pan: Vec2,
        new_scale: f32,
        output: &mut FrameOutput,
    ) {
        if view_graph.scale.ui_equals(new_scale) && view_graph.pan.ui_equals(new_pan) {
            return;
        }
        output.add_action(GraphUiAction::ZoomPanChanged {
            before_pan: view_graph.pan,
            before_scale: view_graph.scale,
            after_pan: new_pan,
            after_scale: new_scale,
        });
    }
}

// ============================================================================
// Pure target-computing functions (used by overlay buttons and scroll handler).
// ============================================================================

/// Given the current `(scale, pan)` and this frame's scroll / zoom input,
/// return the target `(scale, pan)`. No mutation — the orchestrator emits
/// a `ZoomPanChanged` action if the result differs from the current view.
fn compute_scroll_zoom(
    vp: &ViewParams,
    input: &InputSnapshot,
    pointer_pos: Pos2,
    current_scale: f32,
    current_pan: Vec2,
) -> (f32, Vec2) {
    let (scroll_delta, mouse_wheel_delta) = (input.scroll_delta, input.wheel_lines);

    let (zoom_delta, pan_delta) = (mouse_wheel_delta.abs() > f32::EPSILON).then_else(
        ((mouse_wheel_delta * WHEEL_ZOOM_SPEED).exp(), Vec2::ZERO),
        (input.zoom_delta_unless_cmd(), scroll_delta),
    );

    let mut new_scale = current_scale;
    let mut new_pan = current_pan;

    if (zoom_delta - 1.0).abs() > f32::EPSILON {
        // Zoom, pinned to cursor. Skip pan_delta — a wheel tick
        // can produce both a zoom signal and a scroll signal;
        // applying both would push the graph under the cursor.
        let clamped_scale = (current_scale * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
        let origin = vp.rect.min;
        let graph_pos = (pointer_pos - origin - current_pan) / current_scale;
        new_scale = clamped_scale;
        new_pan = pointer_pos - origin - graph_pos * clamped_scale;
    } else {
        new_pan += pan_delta;
    }

    (new_scale, new_pan)
}

/// Computes target `(scale, pan)` that centres on the selected node, or
/// `None` if nothing is selected / the layout isn't known.
pub(super) fn view_selected_node_target(
    vp: &ViewParams,
    ctx: &GraphContext<'_>,
    graph_layout: &GraphLayout,
) -> Option<(f32, Vec2)> {
    let selected_id = ctx.view_graph.selected_node_id?;
    let node_view = ctx.view_graph.view_nodes.by_key(&selected_id)?;

    let scale = ctx.view_graph.scale;
    let rect = graph_layout
        .node_layout(vp, ctx, &node_view.id, Vec2::ZERO)
        .body_rect;
    let size = rect.size() / scale;
    let center = egui::pos2(
        node_view.pos.x + size.x * 0.5,
        node_view.pos.y + size.y * 0.5,
    );

    let target_scale = 1.0;
    let available = vp.rect;
    let target_pan = available.center() - available.min - center.to_vec2();
    Some((target_scale, target_pan))
}

/// Computes target `(scale, pan)` that fits all nodes on screen. Returns
/// `(1.0, Vec2::ZERO)` when the graph is empty.
pub(super) fn fit_all_nodes_target(
    vp: &ViewParams,
    ctx: &GraphContext<'_>,
    graph_layout: &GraphLayout,
) -> (f32, Vec2) {
    if ctx.view_graph.view_nodes.is_empty() {
        return (1.0, Vec2::ZERO);
    }

    let origin = layout::origin(vp, ctx);
    let scale = ctx.view_graph.scale;
    let to_graph_rect = |rect: egui::Rect| {
        let min = (rect.min - origin) / scale;
        let max = (rect.max - origin) / scale;
        egui::Rect::from_min_max(egui::pos2(min.x, min.y), egui::pos2(max.x, max.y))
    };

    let mut layouts = ctx
        .view_graph
        .view_nodes
        .iter()
        .map(|vn| graph_layout.node_layout(vp, ctx, &vn.id, Vec2::ZERO));
    let first = layouts
        .next()
        .expect("view_nodes non-empty — checked above");
    let mut bounds = to_graph_rect(first.body_rect);

    for layout in layouts {
        bounds = bounds.union(to_graph_rect(layout.body_rect));
    }

    let bounds_size = bounds.size();
    let padding = 24.0;
    let gui_rect = vp.rect;
    let available = gui_rect.size() - egui::vec2(padding * 2.0, padding * 2.0);
    let zoom_x = (bounds_size.x > 0.0).then_else(available.x / bounds_size.x, 1.0);
    let zoom_y = (bounds_size.y > 0.0).then_else(available.y / bounds_size.y, 1.0);

    let target_scale = zoom_x.min(zoom_y).clamp(MIN_ZOOM, MAX_ZOOM);
    let bounds_center = bounds.center().to_vec2();
    let target_pan = gui_rect.center() - gui_rect.min - bounds_center * target_scale;
    (target_scale, target_pan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::style::Style;
    use egui::{Pos2, Rect, vec2};
    use std::rc::Rc;

    fn vp_at(rect: Rect, scale: f32) -> ViewParams {
        ViewParams {
            style: Rc::new(Style::default()),
            scale,
            rect,
        }
    }

    /// `egui::InputState::zoom_delta()` returns 1.0 when there's no zoom
    /// gesture this frame, so 1.0 — not 0.0 — is the identity for tests.
    fn idle_input() -> InputSnapshot {
        InputSnapshot {
            zoom_delta: 1.0,
            ..Default::default()
        }
    }

    fn input_with_zoom(zoom: f32) -> InputSnapshot {
        let mut input = idle_input();
        input.zoom_delta = zoom;
        input
    }

    fn input_with_scroll(delta: Vec2) -> InputSnapshot {
        let mut input = idle_input();
        input.scroll_delta = delta;
        input
    }

    /// Scroll-pan with no zoom: pan accumulates the scroll_delta verbatim.
    #[test]
    fn compute_scroll_zoom_pans_when_no_zoom() {
        let vp = vp_at(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 1.0);
        let input = input_with_scroll(vec2(10.0, -5.0));
        let pointer = Pos2::new(400.0, 300.0);

        let (scale, pan) = compute_scroll_zoom(&vp, &input, pointer, 1.0, Vec2::ZERO);

        assert_eq!(scale, 1.0);
        assert_eq!(pan, vec2(10.0, -5.0));
    }

    /// Zoom is pinned to the cursor: the graph point under the pointer
    /// stays under the pointer after the zoom. With pointer at (400,300),
    /// rect at origin, current_pan=ZERO, current_scale=1.0:
    ///   graph_pos = (pointer - origin - pan) / scale = (400, 300)
    /// After zoom_delta=2.0 (so new_scale=2.0):
    ///   new_pan = pointer - origin - graph_pos * new_scale
    ///           = (400, 300) - (0,0) - (400,300)*2 = (-400, -300)
    /// Verifying: pointer = origin + new_pan + graph_pos * new_scale
    ///                    = (0,0) + (-400,-300) + (400,300)*2 = (400,300). ✓
    #[test]
    fn compute_scroll_zoom_pins_to_cursor() {
        let vp = vp_at(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 1.0);
        let input = input_with_zoom(2.0);
        let pointer = Pos2::new(400.0, 300.0);

        let (scale, pan) = compute_scroll_zoom(&vp, &input, pointer, 1.0, Vec2::ZERO);

        assert!((scale - 2.0).abs() < 1e-5, "scale was {scale}");
        assert!((pan.x - -400.0).abs() < 1e-3, "pan.x was {}", pan.x);
        assert!((pan.y - -300.0).abs() < 1e-3, "pan.y was {}", pan.y);
        // Sanity: the point under the cursor in graph-space is unchanged.
        let origin = vp.rect.min;
        let graph_pos_before = (pointer - origin - Vec2::ZERO) / 1.0;
        let graph_pos_after = (pointer - origin - pan) / scale;
        assert!((graph_pos_before - graph_pos_after).length() < 1e-3);
    }

    /// Zoom is clamped to MAX_ZOOM (4.0).
    #[test]
    fn compute_scroll_zoom_clamps_to_max() {
        let vp = vp_at(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 1.0);
        let input = input_with_zoom(1000.0);
        let (scale, _) = compute_scroll_zoom(&vp, &input, Pos2::new(0.0, 0.0), 1.0, Vec2::ZERO);
        assert_eq!(scale, MAX_ZOOM);
    }

    /// Zoom is clamped to MIN_ZOOM (0.2).
    #[test]
    fn compute_scroll_zoom_clamps_to_min() {
        let vp = vp_at(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 1.0);
        let input = input_with_zoom(0.001);
        let (scale, _) = compute_scroll_zoom(&vp, &input, Pos2::new(0.0, 0.0), 1.0, Vec2::ZERO);
        assert_eq!(scale, MIN_ZOOM);
    }

    /// Wheel-line input takes priority over zoom_delta and produces zoom,
    /// not pan. wheel_lines = 1.0 → zoom_delta = exp(WHEEL_ZOOM_SPEED).
    #[test]
    fn compute_scroll_zoom_wheel_line_zooms() {
        let vp = vp_at(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), 1.0);
        let mut input = idle_input();
        input.wheel_lines = 1.0;
        input.scroll_delta = vec2(50.0, 50.0); // ignored — wheel takes precedence
        let (scale, pan) = compute_scroll_zoom(&vp, &input, Pos2::new(0.0, 0.0), 1.0, Vec2::ZERO);
        let expected_scale = (WHEEL_ZOOM_SPEED).exp();
        assert!((scale - expected_scale).abs() < 1e-5, "scale was {scale}");
        // Pointer at origin, current_pan=ZERO → graph_pos=ZERO → new_pan=ZERO.
        assert!(
            pan.length() < 1e-5,
            "wheel-zoom pan should be zero, got {pan:?}"
        );
    }
}
