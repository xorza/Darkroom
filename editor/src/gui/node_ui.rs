use crate::common::font::ScaledFontId;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::node_layout::{NodeLayout, compute_node_layout, text_width};

use common::BoolExt;
use eframe::egui;
use egui::{PointerButton, Pos2, Sense, Vec2, vec2};
use graph::data::StaticValue;
use graph::graph::{Binding, Node, NodeId};
use graph::prelude::{Func, FuncBehavior, NodeBehavior};

use crate::gui::{graph_ctx::GraphContext, graph_ui::GraphUiAction, graph_ui::GraphUiInteraction};
use crate::model::{ViewGraph, ViewNode};

#[derive(Debug, Clone)]
pub enum PortDragInfo {
    None,
    Hover(PortInfo),
    DragStart(PortInfo),
    DragStop,
}

#[derive(Debug, Default)]
pub struct NodeUi {
    node_ids_to_remove: Vec<NodeId>,
}

impl NodeUi {
    pub fn render_nodes(
        &mut self,
        ctx: &mut GraphContext,
        view_graph: &mut ViewGraph,
        graph_layout: &mut GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
    ) -> PortDragInfo {
        self.node_ids_to_remove.clear();
        let mut drag_port_info: PortDragInfo = PortDragInfo::None;

        for view_node_idx in 0..view_graph.view_nodes.len() {
            let node_id = view_graph.view_nodes[view_node_idx].id;
            let node_layout = body_drag(ctx, view_graph, graph_layout, ui_interaction, &node_id);

            let node = view_graph.graph.by_id_mut(&node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();
            let view_node = &view_graph.view_nodes[view_node_idx];

            let is_selected = view_graph.selected_node_id.is_some_and(|id| id == node_id);

            render_body(ctx, &node_layout, is_selected);
            if render_remove_btn(ctx, ui_interaction, &node_id, &node_layout) {
                self.node_ids_to_remove.push(node_id);
            }
            render_cache_btn(ctx, ui_interaction, &node_layout, node);
            render_hints(ctx, &node_layout, node, func);
            let node_drag_port_result = render_node_ports(ctx, &node_layout, view_node, func);
            drag_port_info = drag_port_info.prefer(node_drag_port_result);
            render_node_const_bindings(ctx, &node_layout, node);
            render_node_labels(ctx, &node_layout);
        }

        while let Some(node_id) = self.node_ids_to_remove.pop() {
            view_graph.remove_node(&node_id);
        }

        drag_port_info
    }
}

fn body_drag(
    ctx: &mut GraphContext<'_>,
    view_graph: &mut ViewGraph,
    graph_layout: &mut GraphLayout,
    ui_interaction: &mut GraphUiInteraction,
    node_id: &NodeId,
) -> NodeLayout {
    let node_layout = graph_layout.node_layout(node_id).clone();

    let node_body_id = ctx.ui.make_persistent_id(("node_body", node_id));
    let body_response = ctx.ui.interact(
        node_layout.body_rect,
        node_body_id,
        egui::Sense::click() | egui::Sense::hover() | egui::Sense::drag(),
    );

    let dragged = body_response.dragged_by(PointerButton::Middle)
        || body_response.dragged_by(PointerButton::Primary);

    if dragged || body_response.clicked() {
        ui_interaction
            .actions
            .push((*node_id, GraphUiAction::NodeSelected));
        view_graph.selected_node_id = Some(*node_id);
    }

    if dragged {
        view_graph.view_nodes.by_key_mut(node_id).unwrap().pos +=
            body_response.drag_delta() / ctx.scale;

        let new_layout = compute_node_layout(ctx, view_graph, node_id, graph_layout.origin);
        graph_layout.update_node_layout(node_id, new_layout.clone());
        return new_layout;
    }

    node_layout
}

fn render_body(ctx: &mut GraphContext<'_>, layout: &NodeLayout, selected: bool) {
    let corner_radius = ctx.style.corner_radius * ctx.scale;
    let stroke = selected.then_else(ctx.style.active_bg_stroke, ctx.style.inactive_bg_stroke);
    ctx.painter.rect(
        layout.body_rect,
        corner_radius,
        ctx.style.inactive_bg_fill,
        stroke,
        egui::StrokeKind::Middle,
    );
    let title_pos = layout.body_rect.min + Vec2::ONE * ctx.style.node.padding * ctx.scale;
    ctx.painter
        .galley(title_pos, layout.title_galley.clone(), ctx.style.text_color);
}

fn render_cache_btn(
    ctx: &mut GraphContext,
    ui_interaction: &mut GraphUiInteraction,
    node_layout: &NodeLayout,
    node: &mut Node,
) {
    if ctx.toggle_button(
        node_layout.cache_button_rect,
        "cache",
        !node.terminal,
        node.behavior == NodeBehavior::Once,
        (node.id, "cache"),
        "",
    ) {
        node.behavior = (node.behavior == NodeBehavior::Once)
            .then_else(NodeBehavior::AsFunction, NodeBehavior::Once);
        ui_interaction
            .actions
            .push((node.id, GraphUiAction::CacheToggled));
    };
}

fn render_hints(
    ctx: &mut GraphContext,
    layout: &NodeLayout,
    node: &graph::prelude::Node,
    func: &graph::prelude::Func,
) {
    let dot_radius = ctx.scale * ctx.style.node.status_dot_radius;
    let dot_step = (dot_radius * 2.0) + ctx.scale + ctx.style.node.padding;

    if node.terminal {
        let center = layout.dot_center(0, dot_step);
        ctx.painter
            .circle_filled(center, dot_radius, ctx.style.node.status_terminal_color);
        let dot_rect =
            egui::Rect::from_center_size(center, egui::vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = ctx.ui.make_persistent_id(("node_status_terminal", node.id));
        let dot_response = ctx.ui.interact(dot_rect, dot_id, egui::Sense::hover());
        if dot_response.hovered() {
            dot_response.show_tooltip_text("terminal");
        }
    }
    if func.behavior == FuncBehavior::Impure {
        let center = layout.dot_center(usize::from(node.terminal), dot_step);
        ctx.painter
            .circle_filled(center, dot_radius, ctx.style.node.status_impure_color);
        let dot_rect =
            egui::Rect::from_center_size(center, egui::vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = ctx.ui.make_persistent_id(("node_status_impure", node.id));
        let dot_response = ctx.ui.interact(dot_rect, dot_id, egui::Sense::hover());
        if dot_response.hovered() {
            dot_response.show_tooltip_text("impure");
        }
    }
}

fn render_remove_btn(
    ctx: &mut GraphContext,
    ui_interaction: &mut GraphUiInteraction,
    node_id: &NodeId,
    node_layout: &NodeLayout,
) -> bool {
    let remove_rect = node_layout.remove_btn_rect;
    let remove_margin = remove_rect.width() * 0.3;
    let a = egui::pos2(
        remove_rect.min.x + remove_margin,
        remove_rect.min.y + remove_margin,
    );
    let b = egui::pos2(
        remove_rect.max.x - remove_margin,
        remove_rect.max.y - remove_margin,
    );
    let c = egui::pos2(
        remove_rect.min.x + remove_margin,
        remove_rect.max.y - remove_margin,
    );
    let d = egui::pos2(
        remove_rect.max.x - remove_margin,
        remove_rect.min.y + remove_margin,
    );
    let remove_color = ctx.style.text_color;
    let remove_stroke = egui::Stroke::new(1.4 * ctx.scale, remove_color);
    let remove_shapes = [
        egui::Shape::line_segment([a, b], remove_stroke),
        egui::Shape::line_segment([c, d], remove_stroke),
    ];
    let remove = ctx.button_with(
        remove_rect,
        true,
        ("node_remove", node_id),
        remove_shapes.iter().cloned(),
        "Remove node",
    );

    if remove {
        ui_interaction
            .actions
            .push((*node_id, GraphUiAction::NodeRemoved));
        return true;
    }

    false
}

fn render_node_ports(
    ctx: &GraphContext,
    layout: &NodeLayout,
    view_node: &ViewNode,
    func: &Func,
) -> PortDragInfo {
    let port_radius = ctx.style.node.port_radius * ctx.scale;
    let port_activation_radius = ctx.style.node.port_activation_radius * ctx.scale;
    let port_rect_size = egui::vec2(port_activation_radius * 2.0, port_activation_radius * 2.0);

    let draw_port = |center: Pos2,
                     kind: PortKind,
                     idx: usize,
                     base_color: egui::Color32,
                     hover_color: egui::Color32|
     -> PortDragInfo {
        let port_rect = egui::Rect::from_center_size(center, port_rect_size);
        let port_id = ctx
            .ui
            .make_persistent_id(("node_port", kind, view_node.id, idx));
        let response = ctx
            .ui
            .interact(port_rect, port_id, Sense::drag() | Sense::hover());
        let is_hovered = ctx.ui.rect_contains_pointer(port_rect);

        let color = is_hovered.then_else(hover_color, base_color);
        ctx.painter.circle_filled(center, port_radius, color);

        let port_info = PortInfo {
            port: PortRef {
                node_id: view_node.id,
                idx,
                kind,
            },
            center,
        };
        if response.drag_started_by(PointerButton::Primary) {
            PortDragInfo::DragStart(port_info)
        } else if response.drag_stopped_by(PointerButton::Primary) {
            PortDragInfo::DragStop
        } else if is_hovered {
            PortDragInfo::Hover(port_info)
        } else {
            PortDragInfo::None
        }
    };

    let mut port_drag_info: PortDragInfo = PortDragInfo::None;

    for input_idx in 0..func.inputs.len() {
        let center = layout.input_center(input_idx);
        let drag_info = draw_port(
            center,
            PortKind::Input,
            input_idx,
            ctx.style.node.input_port_color,
            ctx.style.node.input_hover_color,
        );
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    for output_idx in 0..func.outputs.len() {
        let center = layout.output_center(output_idx);
        let drag_info = draw_port(
            center,
            PortKind::Output,
            output_idx,
            ctx.style.node.output_port_color,
            ctx.style.node.output_hover_color,
        );
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    port_drag_info
}

fn render_node_const_bindings(ctx: &mut GraphContext, node_layout: &NodeLayout, node: &Node) {
    // todo refactor styling
    let font = ctx.style.body_font.scaled(ctx.scale);
    let port_radius = ctx.scale * ctx.style.node.port_radius;
    let link_inset = port_radius * 0.6;
    let badge_padding = node_layout.padding;
    let row_height = node_layout.port_row_height;
    let badge_height = (row_height * 1.2).max(10.0 * ctx.scale);
    let badge_radius = 6.0 * ctx.scale;
    let badge_gap = 6.0 * ctx.scale;
    let stroke = ctx.style.connections.stroke;

    for (input_idx, input) in node.inputs.iter().enumerate() {
        let Binding::Const(value) = &input.binding else {
            continue;
        };

        let label = static_value_label(value);
        let label_width = text_width(&ctx.painter, &font, &label, ctx.style.text_color);
        let badge_width = label_width + badge_padding * 2.0;
        let center = node_layout.input_center(input_idx);
        let badge_right = center.x - port_radius - badge_gap;
        let badge_rect = egui::Rect::from_min_max(
            egui::pos2(badge_right - badge_width, center.y - badge_height * 0.5),
            egui::pos2(badge_right, center.y + badge_height * 0.5),
        );

        let link_start = egui::pos2(badge_rect.max.x, center.y);
        let link_end = egui::pos2(center.x - link_inset, center.y);
        ctx.painter.line_segment([link_start, link_end], stroke);
        ctx.painter.rect(
            badge_rect,
            badge_radius,
            ctx.style.inactive_bg_fill,
            ctx.style.node.const_stroke,
            egui::StrokeKind::Middle,
        );
        ctx.painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            ctx.style.text_color,
        );
    }
}

fn static_value_label(value: &StaticValue) -> String {
    match value {
        StaticValue::Null => "null".to_string(),
        StaticValue::Float(value) => (value.fract() == 0.0)
            .then_else_with(|| format!("{:.0}", value), || format!("{:.3}", value)),
        StaticValue::Int(value) => value.to_string(),
        StaticValue::Bool(value) => value.to_string(),
        StaticValue::String(value) => {
            const MAX_LEN: usize = 12;
            if value.chars().count() <= MAX_LEN {
                value.clone()
            } else {
                let truncated: String = value.chars().take(MAX_LEN).collect();
                format!("{}...", truncated)
            }
        }
    }
}

fn render_node_labels(ctx: &mut GraphContext, node_layout: &NodeLayout) {
    let padding = ctx.style.node.port_label_side_padding * ctx.scale;

    for (input_idx, galley) in node_layout.input_galleys.iter().enumerate() {
        let text_pos = node_layout.input_center(input_idx) + vec2(padding, -galley.size().y * 0.5);
        ctx.painter
            .galley(text_pos, galley.clone(), ctx.style.text_color);
    }

    for (output_idx, galley) in node_layout.output_galleys.iter().enumerate() {
        let text_pos = node_layout.output_center(output_idx)
            + vec2(-padding - galley.size().x, -galley.size().y * 0.5);
        ctx.painter
            .galley(text_pos, galley.clone(), ctx.style.text_color);
    }
}

impl PortDragInfo {
    fn prio(&self) -> u32 {
        match self {
            PortDragInfo::None => 0,
            PortDragInfo::Hover(_) => 5,
            PortDragInfo::DragStart(_) => 8,
            PortDragInfo::DragStop => 10,
        }
    }

    fn prefer(self, other: Self) -> Self {
        (other.prio() > self.prio()).then_else(other, self)
    }
}
