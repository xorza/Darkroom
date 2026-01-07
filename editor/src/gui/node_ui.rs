use crate::common::font::ScaledFontId;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};

use common::BoolExt;
use eframe::egui;
use egui::{PointerButton, Pos2, Rect, Sense, vec2};
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

#[derive(Debug, Clone)]
pub struct NodeLayout {
    pub rect: Rect,
    pub remove_btn_rect: Rect,
    pub cache_button_rect: Rect,
    pub dot_first_center: Option<Pos2>,
    pub input_first_center: Pos2,
    pub output_first_center: Pos2,
}

#[derive(Debug, Default)]
pub struct NodeUi {
    node_ids_to_remove: Vec<NodeId>,
}

impl NodeLayout {
    pub fn input_center(&self, index: usize, row_height: f32) -> Pos2 {
        egui::pos2(
            self.input_first_center.x,
            self.input_first_center.y + row_height * index as f32,
        )
    }

    pub fn output_center(&self, index: usize, row_height: f32) -> Pos2 {
        egui::pos2(
            self.output_first_center.x,
            self.output_first_center.y + row_height * index as f32,
        )
    }

    pub fn dot_center(&self, index: usize, dot_step: f32) -> Pos2 {
        let first = self
            .dot_first_center
            .expect("dot center missing when dots are present");
        egui::pos2(first.x - dot_step * index as f32, first.y)
    }
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

            render_body(ctx, node, &node_layout, is_selected, view_graph.scale);
            if render_remove_btn(
                ctx,
                ui_interaction,
                &node_id,
                &node_layout,
                view_graph.scale,
            ) {
                self.node_ids_to_remove.push(node_id);
            }
            render_cache_btn(ctx, ui_interaction, &node_layout, node);
            render_hints(ctx, &node_layout, node, func, view_graph.scale);
            let node_drag_port_result =
                render_node_ports(ctx, &node_layout, view_node, func, view_graph.scale);
            drag_port_info = drag_port_info.prefer(node_drag_port_result);
            render_node_const_bindings(ctx, &node_layout, node, view_graph.scale);
            render_node_labels(ctx, view_graph, &node_layout, func);
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
        node_layout.rect,
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
            body_response.drag_delta() / view_graph.scale;

        let new_layout = compute_node_layout(ctx, view_graph, node_id, graph_layout.origin);
        graph_layout.update_node_layout(node_id, new_layout.clone());
        return new_layout;
    }

    node_layout
}

fn render_body(
    ctx: &mut GraphContext<'_>,
    node: &mut Node,
    layout: &NodeLayout,
    selected: bool,
    scale: f32,
) {
    let corner_radius = ctx.style.node_corner_radius * scale;
    ctx.painter.rect(
        layout.rect,
        corner_radius,
        ctx.style.node_fill,
        selected.then_else(ctx.style.selected_stroke, ctx.style.node_stroke),
        egui::StrokeKind::Inside,
    );
    ctx.painter.text(
        layout.rect.min
            + egui::vec2(
                ctx.style.node_padding * scale,
                scale * ctx.style.header_text_offset,
            ),
        egui::Align2::LEFT_TOP,
        &mut node.name,
        ctx.style.heading_font.scaled(scale),
        ctx.style.text_color,
    );
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
    scale: f32,
) {
    let dot_radius = scale * ctx.style.status_dot_radius;
    let dot_step = (dot_radius * 2.0) + scale * ctx.style.status_item_gap;

    if node.terminal {
        let center = layout.dot_center(0, dot_step);
        ctx.painter
            .circle_filled(center, dot_radius, ctx.style.status_terminal_color);
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
            .circle_filled(center, dot_radius, ctx.style.status_impure_color);
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
    scale: f32,
) -> bool {
    let remove_btn_id = ctx.ui.make_persistent_id(("node_remove", node_id));

    let remove_response = ctx.ui.interact(
        node_layout.remove_btn_rect,
        remove_btn_id,
        egui::Sense::click(),
    );
    if remove_response.hovered() {
        remove_response.show_tooltip_text("Remove node");
    }

    if remove_response.clicked() {
        ui_interaction
            .actions
            .push((*node_id, GraphUiAction::NodeRemoved));
        return true;
    }

    let close_fill = if remove_response.is_pointer_button_down_on() {
        ctx.style.widget_active_bg_fill
    } else if remove_response.hovered() {
        ctx.style.widget_hover_bg_fill
    } else {
        ctx.style.widget_inactive_bg_fill
    };
    let close_stroke = ctx.style.widget_inactive_bg_stroke;
    ctx.painter.rect(
        node_layout.remove_btn_rect,
        ctx.style.node_corner_radius * scale * 0.6,
        close_fill,
        close_stroke,
        egui::StrokeKind::Inside,
    );
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
    let remove_color = ctx.style.widget_text_color;
    let remove_stroke = egui::Stroke::new(1.4 * scale, remove_color);
    ctx.painter.line_segment([a, b], remove_stroke);
    ctx.painter.line_segment([c, d], remove_stroke);

    false
}

fn render_node_ports(
    ctx: &GraphContext,
    layout: &NodeLayout,
    view_node: &ViewNode,
    func: &Func,
    scale: f32,
) -> PortDragInfo {
    assert!(scale > 0.0, "node port scale must be positive");

    let port_radius = scale * ctx.style.port_radius;
    let port_rect_size = egui::vec2(port_radius * 2.0, port_radius * 2.0);
    let row_height = ctx.style.node_row_height * scale;

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
            .interact(port_rect, port_id, Sense::hover() | Sense::drag());
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
        let center = layout.input_center(input_idx, row_height);
        let drag_info = draw_port(
            center,
            PortKind::Input,
            input_idx,
            ctx.style.input_port_color,
            ctx.style.input_hover_color,
        );
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    for output_idx in 0..func.outputs.len() {
        let center = layout.output_center(output_idx, row_height);
        let drag_info = draw_port(
            center,
            PortKind::Output,
            output_idx,
            ctx.style.output_port_color,
            ctx.style.output_hover_color,
        );
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    port_drag_info
}

fn render_node_const_bindings(
    ctx: &mut GraphContext,
    layout: &NodeLayout,
    node: &Node,
    scale: f32,
) {
    let font = ctx.style.body_font.scaled(scale);
    let port_radius = scale * ctx.style.port_radius;
    let link_inset = port_radius * 0.6;
    let badge_padding = 4.0 * scale;
    let row_height = ctx.style.node_row_height * scale;
    let badge_height = (row_height * 1.2).max(10.0 * scale);
    let badge_radius = 6.0 * scale;
    let badge_gap = 6.0 * scale;
    let stroke = ctx.style.connection_stroke;

    for (input_idx, input) in node.inputs.iter().enumerate() {
        let Binding::Const(value) = &input.binding else {
            continue;
        };

        let label = static_value_label(value);
        let label_width = text_width(&ctx.painter, &font, &label, ctx.style.text_color);
        let badge_width = label_width + badge_padding * 2.0;
        let center = layout.input_center(input_idx, row_height);
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
            ctx.style.node_fill,
            ctx.style.const_stroke,
            egui::StrokeKind::Inside,
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

fn render_node_labels(
    ctx: &mut GraphContext,
    view_graph: &ViewGraph,
    layout: &NodeLayout,
    func: &Func,
) {
    let row_height = ctx.style.node_row_height * view_graph.scale;
    let padding = ctx.style.node_padding * view_graph.scale;
    for (input_idx, input) in func.inputs.iter().enumerate() {
        let text_pos = layout.input_center(input_idx, row_height) + vec2(padding, 0.0);
        ctx.painter.text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            &input.name,
            ctx.style.body_font.scaled(view_graph.scale),
            ctx.style.text_color,
        );
    }

    for (output_idx, output) in func.outputs.iter().enumerate() {
        let text_pos = layout.output_center(output_idx, row_height) - vec2(padding, 0.0);
        ctx.painter.text(
            text_pos,
            egui::Align2::RIGHT_CENTER,
            &output.name,
            ctx.style.body_font.scaled(view_graph.scale),
            ctx.style.text_color,
        );
    }
}

pub(crate) fn compute_node_layout(
    ctx: &GraphContext,
    view_graph: &ViewGraph,
    view_node_id: &NodeId,
    origin: Pos2,
) -> NodeLayout {
    let view_node = view_graph.view_nodes.by_key(view_node_id).unwrap();
    let node = view_graph.graph.by_id(view_node_id).unwrap();
    let func = ctx.func_lib.by_id(&node.func_id).unwrap();
    let scale = view_graph.scale;

    let node_width_base = ctx.style.node_width * scale;
    let header_height = ctx.style.node_header_height * scale;
    let cache_height = ctx.style.node_cache_height * scale;
    let row_height = ctx.style.node_row_height * scale;
    let padding = ctx.style.node_padding * scale;

    let header_width = text_width(
        &ctx.painter,
        &ctx.style.heading_font.scaled(scale),
        &node.name,
        ctx.style.text_color,
    ) + padding * 2.0;
    let vertical_padding = padding * ctx.style.cache_button_vertical_pad_factor;
    let cache_text_width = text_width(
        &ctx.painter,
        &ctx.style.body_font.scaled(scale),
        "cache",
        ctx.style.text_color,
    );
    let cache_button_height = (cache_height - vertical_padding * 2.0)
        .max(10.0 * scale)
        .min(cache_height);
    let cache_button_width = (cache_button_height * ctx.style.cache_button_width_factor)
        .max(cache_button_height)
        .max(cache_text_width + padding * ctx.style.cache_button_text_pad_factor * 2.0);
    let cache_row_width = padding + cache_button_width + padding;
    let status_row_width = {
        let dot_diameter = ctx.style.status_dot_radius * 2.0;
        let count = 2usize;
        let gaps = (count - 1) as f32;
        let total = count as f32 * dot_diameter + gaps * ctx.style.status_item_gap;
        padding + total + padding
    };

    let input_count = func.inputs.len();
    let output_count = func.outputs.len();
    let row_count = input_count.max(output_count).max(1);
    let mut max_row_width: f32 = 0.0;

    let inter_side_padding = 0.0;
    for row in 0..row_count {
        let left = func.inputs.get(row).map_or(0.0, |input| {
            text_width(
                &ctx.painter,
                &ctx.style.body_font.scaled(scale),
                &input.name,
                ctx.style.text_color,
            )
        });
        let right = func.outputs.get(row).map_or(0.0, |output| {
            text_width(
                &ctx.painter,
                &ctx.style.body_font.scaled(scale),
                &output.name,
                ctx.style.text_color,
            )
        });
        let mut row_width = padding * 2.0 + left + right;
        if left > 0.0 && right > 0.0 {
            row_width += inter_side_padding;
        }
        max_row_width = max_row_width.max(row_width);
    }

    let node_width = node_width_base
        .max(header_width)
        .max(max_row_width)
        .max(cache_row_width)
        .max(status_row_width);

    let height = header_height + cache_height + padding + row_height * row_count as f32 + padding;
    let node_size = egui::vec2(node_width, height);

    let rect = egui::Rect::from_min_size(Pos2::ZERO, node_size);
    let header_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), header_height));
    let cache_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(0.0, header_height),
        egui::vec2(rect.width(), cache_height),
    );
    let close_size = (header_height - padding)
        .max(12.0 * scale)
        .min(header_height);
    let close_pos = egui::pos2(
        rect.max.x - padding - close_size,
        rect.min.y + (header_height - close_size) * 0.5,
    );
    let close_rect = egui::Rect::from_min_size(close_pos, egui::vec2(close_size, close_size));

    let dot_radius = scale * ctx.style.status_dot_radius;
    let has_terminal = node.terminal;
    let has_impure = func.behavior == FuncBehavior::Impure;
    let dot_first_center = if has_terminal || has_impure {
        let dot_x = close_rect.min.x - padding - dot_radius;
        let dot_center_y = header_rect.center().y;
        Some(egui::pos2(dot_x, dot_center_y))
    } else {
        None
    };

    let cache_button_rect = if cache_height > 0.0 {
        let cache_button_pos = egui::pos2(
            cache_rect.min.x + padding,
            cache_rect.min.y + (cache_height - cache_button_height) * 0.5,
        );
        egui::Rect::from_min_size(
            cache_button_pos,
            egui::vec2(cache_button_width, cache_button_height),
        )
    } else {
        egui::Rect::from_min_size(cache_rect.min, egui::Vec2::ZERO)
    };

    let base_y = rect.min.y + header_height + cache_height + padding + row_height * 0.5;
    let input_first_center = egui::pos2(rect.min.x, base_y);
    let output_first_center = egui::pos2(rect.min.x + node_width, base_y);

    let offset = origin + view_node.pos.to_vec2() * scale;
    let delta = offset.to_vec2();
    let rect = rect.translate(delta);
    let remove_btn_rect = close_rect.translate(delta);
    let cache_button_rect = cache_button_rect.translate(delta);
    let dot_first_center = dot_first_center.map(|center| center + delta);
    let input_first_center = input_first_center + delta;
    let output_first_center = output_first_center + delta;

    NodeLayout {
        rect,
        remove_btn_rect,
        cache_button_rect,
        dot_first_center,
        input_first_center,
        output_first_center,
    }
}

fn text_width(
    painter: &egui::Painter,
    font: &egui::FontId,
    text: &str,
    color: egui::Color32,
) -> f32 {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    galley.size().x
}
