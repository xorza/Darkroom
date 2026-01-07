use crate::common::font::ScaledFontId;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::node_layout::NodeLayout;

use common::BoolExt;
use eframe::egui;
use egui::{
    Color32, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use graph::data::StaticValue;
use graph::graph::{Binding, Node, NodeId};
use graph::prelude::{FuncBehavior, NodeBehavior};

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

            render_body(ctx, node_layout, is_selected);
            if render_remove_btn(ctx, ui_interaction, &node_id, node_layout) {
                self.node_ids_to_remove.push(node_id);
            }
            render_cache_btn(ctx, ui_interaction, node_layout, node);
            render_hints(ctx, node_layout, node, func);
            render_node_const_bindings(ctx, node_layout, node);
            let node_drag_port_result = render_node_ports(ctx, node_layout, view_node);
            drag_port_info = drag_port_info.prefer(node_drag_port_result);
            render_node_labels(ctx, node_layout);
        }

        while let Some(node_id) = self.node_ids_to_remove.pop() {
            view_graph.remove_node(&node_id);
        }

        drag_port_info
    }
}

fn body_drag<'a>(
    ctx: &mut GraphContext<'_>,
    view_graph: &mut ViewGraph,
    graph_layout: &'a mut GraphLayout,
    ui_interaction: &mut GraphUiInteraction,
    node_id: &NodeId,
) -> &'a NodeLayout {
    let node_layout = graph_layout.node_layouts.by_key_mut(node_id).unwrap();

    let node_body_id = ctx.ui.make_persistent_id(("node_body", node_id));
    let body_response = ctx.ui.interact(
        node_layout.body_rect,
        node_body_id,
        Sense::click() | Sense::hover() | Sense::drag(),
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

        node_layout.update(ctx, view_graph, graph_layout.origin);
    }

    node_layout
}

fn render_body(ctx: &mut GraphContext<'_>, layout: &NodeLayout, selected: bool) {
    let corner_radius = ctx.style.corner_radius * ctx.scale;
    let stroke = selected.then_else(ctx.style.active_bg_stroke, ctx.style.inactive_bg_stroke);
    ctx.painter.rect(
        layout.body_rect,
        corner_radius,
        ctx.style.noninteractive_bg_fill,
        stroke,
        StrokeKind::Middle,
    );
    let title_pos = layout.body_rect.min + Vec2::ONE * ctx.style.padding * ctx.scale;
    ctx.painter
        .galley(title_pos, layout.title_galley.clone(), ctx.style.text_color);
}

fn render_cache_btn(
    ctx: &mut GraphContext,
    ui_interaction: &mut GraphUiInteraction,
    node_layout: &NodeLayout,
    node: &mut Node,
) {
    let enabled = !node.terminal;
    let checked = node.behavior == NodeBehavior::Once;

    if ctx.toggle_button(
        node_layout.cache_button_rect,
        enabled,
        checked,
        (node.id, "cache"),
        "cache",
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
    node_layout: &NodeLayout,
    node: &graph::prelude::Node,
    func: &graph::prelude::Func,
) {
    let dot_radius = ctx.scale * ctx.style.node.status_dot_radius;
    let dot_step = (dot_radius * 2.0) + ctx.scale + ctx.style.padding;

    if node.terminal {
        let center = node_layout.dot_center(0, dot_step);
        ctx.painter
            .circle_filled(center, dot_radius, ctx.style.node.status_terminal_color);
        let dot_rect =
            egui::Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = ctx.ui.make_persistent_id(("node_status_terminal", node.id));
        let dot_response = ctx.ui.interact(dot_rect, dot_id, Sense::hover());
        if dot_response.hovered() {
            dot_response.show_tooltip_text("terminal");
        }
    }
    if node.behavior == NodeBehavior::AsFunction && func.behavior == FuncBehavior::Impure {
        let center = node_layout.dot_center(usize::from(node.terminal), dot_step);
        ctx.painter
            .circle_filled(center, dot_radius, ctx.style.node.status_impure_color);
        let dot_rect = Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = ctx.ui.make_persistent_id(("node_status_impure", node.id));
        let dot_response = ctx.ui.interact(dot_rect, dot_id, Sense::hover());
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
    let a = pos2(
        remove_rect.min.x + remove_margin,
        remove_rect.min.y + remove_margin,
    );
    let b = pos2(
        remove_rect.max.x - remove_margin,
        remove_rect.max.y - remove_margin,
    );
    let c = pos2(
        remove_rect.min.x + remove_margin,
        remove_rect.max.y - remove_margin,
    );
    let d = pos2(
        remove_rect.max.x - remove_margin,
        remove_rect.min.y + remove_margin,
    );
    let remove_color = ctx.style.text_color;
    let remove_stroke = Stroke::new(1.4 * ctx.scale, remove_color);
    let remove_shapes = [
        Shape::line_segment([a, b], remove_stroke),
        Shape::line_segment([c, d], remove_stroke),
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
    node_layout: &NodeLayout,
    view_node: &ViewNode,
) -> PortDragInfo {
    let port_radius = ctx.style.node.port_radius * ctx.scale;
    let port_activation_radius = ctx.style.node.port_activation_radius * ctx.scale;
    let port_rect_size = vec2(port_activation_radius * 2.0, port_activation_radius * 2.0);

    let draw_port = |center: Pos2,
                     kind: PortKind,
                     idx: usize,
                     base_color: Color32,
                     hover_color: Color32|
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

    for input_idx in 0..node_layout.input_galleys.len() {
        let center = node_layout.input_center(input_idx);
        let drag_info = draw_port(
            center,
            PortKind::Input,
            input_idx,
            ctx.style.node.input_port_color,
            ctx.style.node.input_hover_color,
        );
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    for output_idx in 0..node_layout.output_galleys.len() {
        let center = node_layout.output_center(output_idx);
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

fn render_node_const_bindings(ctx: &mut GraphContext, node_layout: &NodeLayout, node: &Node) {
    // todo refactor styling
    let font = ctx.style.sub_font.scaled(ctx.scale);
    let port_radius = ctx.style.node.port_radius * ctx.scale;

    let padding = node_layout.padding;
    let row_height = node_layout.port_row_height;

    for (input_idx, input) in node.inputs.iter().enumerate() {
        let Binding::Const(value) = &input.binding else {
            continue;
        };

        let label = static_value_label(value);
        let label_galley = ctx
            .painter
            .layout_no_wrap(label, font.clone(), ctx.style.text_color);
        let badge_width = label_galley.size().x + padding * 2.0;
        let input_center = node_layout.input_center(input_idx);
        let badge_right = input_center.x - port_radius - padding;
        let badge_rect = egui::Rect::from_min_max(
            egui::pos2(badge_right - badge_width, input_center.y - row_height * 0.5),
            egui::pos2(badge_right, input_center.y + row_height * 0.5),
        );

        let link_start = egui::pos2(badge_rect.max.x, input_center.y);
        let link_end = egui::pos2(input_center.x - port_radius, input_center.y);

        ctx.painter
            .line_segment([link_start, link_end], ctx.style.connections.stroke);
        ctx.painter.rect(
            badge_rect,
            ctx.style.corner_radius * ctx.scale,
            ctx.style.inactive_bg_fill,
            ctx.style.node.const_stroke,
            StrokeKind::Inside,
        );
        ctx.painter.galley(
            badge_rect.center() - label_galley.size() * 0.5,
            label_galley,
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
