//! Pan and zoom handling for the graph view.
//!
//! Everything in here is a pure projection of `(state, input)` → new
//! target `(scale, pan)`, plus the state-machine transitions for the
//! middle-button pan mode. Mutation of `view_graph.pan` / `scale` happens
//! exclusively through `ZoomPanChanged::apply` in `Session::commit_actions`.

use common::BoolExt;
use egui::{PointerButton, Pos2, Response, Vec2};

use crate::common::UiEquals;
use crate::gui::Gui;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::gui::graph_ui::gesture::Gesture;
use crate::gui::graph_ui::layout::{self, GraphLayout};
use crate::gui::graph_ui::{GraphUi, MAX_ZOOM, MIN_ZOOM, WHEEL_ZOOM_SPEED};
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
            (new_scale, new_pan) = compute_scroll_zoom(gui, input, pointer_pos, new_scale, new_pan);
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
    gui: &Gui<'_>,
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
        let origin = gui.rect.min;
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
    gui: &Gui<'_>,
    ctx: &GraphContext<'_>,
    graph_layout: &GraphLayout,
) -> Option<(f32, Vec2)> {
    let selected_id = ctx.view_graph.selected_node_id?;
    let node_view = ctx.view_graph.view_nodes.by_key(&selected_id)?;

    let scale = ctx.view_graph.scale;
    let rect = graph_layout
        .node_layout(gui, ctx, &node_view.id, Vec2::ZERO)
        .body_rect;
    let size = rect.size() / scale;
    let center = egui::pos2(
        node_view.pos.x + size.x * 0.5,
        node_view.pos.y + size.y * 0.5,
    );

    let target_scale = 1.0;
    let available = gui.rect;
    let target_pan = available.center() - available.min - center.to_vec2();
    Some((target_scale, target_pan))
}

/// Computes target `(scale, pan)` that fits all nodes on screen. Returns
/// `(1.0, Vec2::ZERO)` when the graph is empty.
pub(super) fn fit_all_nodes_target(
    gui: &Gui<'_>,
    ctx: &GraphContext<'_>,
    graph_layout: &GraphLayout,
) -> (f32, Vec2) {
    if ctx.view_graph.view_nodes.is_empty() {
        return (1.0, Vec2::ZERO);
    }

    let origin = layout::origin(gui, ctx);
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
        .map(|vn| graph_layout.node_layout(gui, ctx, &vn.id, Vec2::ZERO));
    let first = layouts
        .next()
        .expect("view_nodes non-empty — checked above");
    let mut bounds = to_graph_rect(first.body_rect);

    for layout in layouts {
        bounds = bounds.union(to_graph_rect(layout.body_rect));
    }

    let bounds_size = bounds.size();
    let padding = 24.0;
    let gui_rect = gui.rect;
    let available = gui_rect.size() - egui::vec2(padding * 2.0, padding * 2.0);
    let zoom_x = (bounds_size.x > 0.0).then_else(available.x / bounds_size.x, 1.0);
    let zoom_y = (bounds_size.y > 0.0).then_else(available.y / bounds_size.y, 1.0);

    let target_scale = zoom_x.min(zoom_y).clamp(MIN_ZOOM, MAX_ZOOM);
    let bounds_center = bounds.center().to_vec2();
    let target_pan = gui_rect.center() - gui_rect.min - bounds_center * target_scale;
    (target_scale, target_pan)
}
