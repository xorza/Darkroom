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
pub struct NodeUi {}

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
    pub fn process_input(
        &mut self,
        ctx: &mut GraphContext,
        view_graph: &mut ViewGraph,
        graph_layout: &mut GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
    ) -> PortDragInfo {
        let mut drag_port_info: PortDragInfo = PortDragInfo::None;

        for view_node_idx in 0..view_graph.view_nodes.len() {
            let view_node_id = view_graph.view_nodes[view_node_idx].id;
            let node_rect = graph_layout.node_rect(&view_node_id);

            let node_ui_id = ctx.ui.make_persistent_id(("node_body", view_node_id));
            let body_response = ctx.ui.interact(
                node_rect,
                node_ui_id,
                egui::Sense::click() | egui::Sense::hover() | egui::Sense::drag(),
            );

            let dragged = body_response.dragged_by(PointerButton::Middle)
                || body_response.dragged_by(PointerButton::Primary);
            if dragged {
                view_graph.view_nodes[view_node_idx].pos +=
                    body_response.drag_delta() / view_graph.scale;

                let new_layout =
                    compute_node_layout(ctx, view_graph, &view_node_id, graph_layout.origin);
                graph_layout.update_node_layout(&view_node_id, new_layout);
            }
            if dragged || body_response.clicked() {
                ui_interaction
                    .actions
                    .push((view_node_id, GraphUiAction::NodeSelected));
                view_graph.selected_node_id = Some(view_node_id);
            }

            let layout = graph_layout.node_layout(&view_node_id);
            let node = view_graph.graph.by_id_mut(&view_node_id).unwrap();

            let close_id = ctx.ui.make_persistent_id(("node_close", node.id));
            let remove_response =
                ctx.ui
                    .interact(layout.remove_btn_rect, close_id, egui::Sense::click());

            let cache_id = ctx.ui.make_persistent_id(("node_cache", node.id));
            let cache_response = ctx.ui.interact(
                layout.cache_button_rect,
                cache_id,
                if !node.terminal {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            );

            if !node.terminal && cache_response.clicked() {
                node.behavior = (node.behavior == NodeBehavior::Once)
                    .then_else(NodeBehavior::AsFunction, NodeBehavior::Once);
                ui_interaction
                    .actions
                    .push((view_node_id, GraphUiAction::CacheToggled));
            }

            if remove_response.hovered() {
                remove_response.show_tooltip_text("Remove node");
            }

            if remove_response.clicked() {
                ui_interaction
                    .actions
                    .push((view_node_id, GraphUiAction::NodeRemoved));
                view_graph.remove_node(&view_node_id);
                continue;
            }

            let view_node = &view_graph.view_nodes[view_node_idx];
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();
            let node_drag_port_result =
                interact_node_ports(ctx, layout, view_node, func, view_graph.scale);
            drag_port_info = drag_port_info.prefer(node_drag_port_result);
        }

        drag_port_info
    }

    pub fn render_nodes(
        &mut self,
        ctx: &mut GraphContext,
        view_graph: &mut ViewGraph,
        graph_layout: &mut GraphLayout,
    ) {
        for view_node_idx in 0..view_graph.view_nodes.len() {
            let view_node = &view_graph.view_nodes[view_node_idx];
            let view_node_id = view_graph.view_nodes[view_node_idx].id;
            let layout = graph_layout.node_layout(&view_node_id);

            let node = view_graph.graph.by_id_mut(&view_node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();

            let close_id = ctx.ui.make_persistent_id(("node_close", node.id));
            let remove_response =
                ctx.ui
                    .interact(layout.remove_btn_rect, close_id, egui::Sense::click());

            let cache_id = ctx.ui.make_persistent_id(("node_cache", node.id));
            let cache_response = ctx.ui.interact(
                layout.cache_button_rect,
                cache_id,
                if !node.terminal {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            );

            let is_selected = view_graph
                .selected_node_id
                .is_some_and(|id| id == view_node_id);

            let corner_radius = ctx.style.node_corner_radius * view_graph.scale;
            ctx.painter.rect(
                layout.rect,
                corner_radius,
                ctx.style.node_fill,
                is_selected.then_else(ctx.style.selected_stroke, ctx.style.node_stroke),
                egui::StrokeKind::Inside,
            );
            ctx.painter.text(
                layout.rect.min
                    + egui::vec2(
                        ctx.style.node_padding * view_graph.scale,
                        view_graph.scale * ctx.style.header_text_offset,
                    ),
                egui::Align2::LEFT_TOP,
                &mut node.name,
                ctx.style.heading_font.scaled(view_graph.scale),
                ctx.style.text_color,
            );

            cache_btn(ctx, layout, node, cache_response, view_graph.scale);
            hints(ctx, layout, node, func, view_graph.scale);
            remove_btn(ctx, layout, remove_response, view_graph.scale);

            render_node_ports(ctx, layout, view_node, func, view_graph.scale);

            render_node_const_bindings(ctx, layout, node, view_graph.scale);
            render_node_labels(ctx, view_graph, layout, func);
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

fn cache_btn(
    ctx: &mut GraphContext,
    layout: &NodeLayout,
    node: &graph::prelude::Node,
    cache_response: egui::Response,
    scale: f32,
) {
    let cache_button_fill = if node.terminal {
        ctx.style.widget_noninteractive_bg_fill
    } else if node.behavior == NodeBehavior::Once {
        ctx.style.cache_active_color
    } else if cache_response.is_pointer_button_down_on() {
        ctx.style.widget_active_bg_fill
    } else if cache_response.hovered() {
        ctx.style.widget_hover_bg_fill
    } else {
        ctx.style.widget_inactive_bg_fill
    };
    let button_stroke = ctx.style.widget_inactive_bg_stroke;
    ctx.painter.rect(
        layout.cache_button_rect,
        ctx.style.node_corner_radius * scale * 0.5,
        cache_button_fill,
        button_stroke,
        egui::StrokeKind::Inside,
    );

    let button_text_color = if node.terminal {
        ctx.style.widget_noninteractive_text_color
    } else if node.behavior == NodeBehavior::Once {
        ctx.style.cache_checked_text_color
    } else {
        ctx.style.widget_text_color
    };
    ctx.painter.text(
        layout.cache_button_rect.center(),
        egui::Align2::CENTER_CENTER,
        "cache",
        ctx.style.body_font.scaled(scale),
        button_text_color,
    );
}

fn hints(
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

fn remove_btn(
    ctx: &mut GraphContext,
    layout: &NodeLayout,
    remove_response: egui::Response,
    scale: f32,
) {
    let close_fill = if remove_response.is_pointer_button_down_on() {
        ctx.style.widget_active_bg_fill
    } else if remove_response.hovered() {
        ctx.style.widget_hover_bg_fill
    } else {
        ctx.style.widget_inactive_bg_fill
    };
    let close_stroke = ctx.style.widget_inactive_bg_stroke;
    ctx.painter.rect(
        layout.remove_btn_rect,
        ctx.style.node_corner_radius * scale * 0.6,
        close_fill,
        close_stroke,
        egui::StrokeKind::Inside,
    );
    let close_rect = layout.remove_btn_rect;
    let close_margin = close_rect.width() * 0.3;
    let a = egui::pos2(
        close_rect.min.x + close_margin,
        close_rect.min.y + close_margin,
    );
    let b = egui::pos2(
        close_rect.max.x - close_margin,
        close_rect.max.y - close_margin,
    );
    let c = egui::pos2(
        close_rect.min.x + close_margin,
        close_rect.max.y - close_margin,
    );
    let d = egui::pos2(
        close_rect.max.x - close_margin,
        close_rect.min.y + close_margin,
    );
    let close_color = ctx.style.widget_text_color;
    let close_stroke = egui::Stroke::new(1.4 * scale, close_color);
    ctx.painter.line_segment([a, b], close_stroke);
    ctx.painter.line_segment([c, d], close_stroke);
}

fn interact_node_ports(
    ctx: &GraphContext,
    layout: &NodeLayout,
    view_node: &ViewNode,
    func: &Func,
    scale: f32,
) -> PortDragInfo {
    let mut port_drag_info: PortDragInfo = PortDragInfo::None;

    let row_height = ctx.style.node_row_height * scale;

    let handle_port = |center: Pos2, kind: PortKind, idx: usize| -> PortDragInfo {
        let port_radius = scale * ctx.style.port_radius;
        let port_rect_size = egui::vec2(port_radius * 2.0, port_radius * 2.0);
        let port_rect = egui::Rect::from_center_size(center, port_rect_size);
        let graph_bg_id = ctx
            .ui
            .make_persistent_id(("node_port", kind, view_node.id, idx));
        let response = ctx
            .ui
            .interact(port_rect, graph_bg_id, Sense::hover() | Sense::drag());
        let is_hovered = ctx.ui.rect_contains_pointer(port_rect);

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

    for input_idx in 0..func.inputs.len() {
        let center = layout.input_center(input_idx, row_height);
        let drag_info = handle_port(center, PortKind::Input, input_idx);
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    for output_idx in 0..func.outputs.len() {
        let center = layout.output_center(output_idx, row_height);
        let drag_info = handle_port(center, PortKind::Output, output_idx);
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    port_drag_info
}

fn render_node_ports(
    ctx: &GraphContext,
    layout: &NodeLayout,
    view_node: &ViewNode,
    func: &Func,
    scale: f32,
) {
    let port_radius = scale * ctx.style.port_radius;
    let port_rect_size = egui::vec2(port_radius * 2.0, port_radius * 2.0);
    let row_height = ctx.style.node_row_height * scale;

    let draw_port = |center: Pos2,
                     kind: PortKind,
                     idx: usize,
                     base_color: egui::Color32,
                     hover_color: egui::Color32| {
        let port_rect = egui::Rect::from_center_size(center, port_rect_size);
        let _port_id = ctx
            .ui
            .make_persistent_id(("node_port", kind, view_node.id, idx));
        let is_hovered = ctx.ui.rect_contains_pointer(port_rect);
        let color = is_hovered.then_else(hover_color, base_color);
        ctx.painter.circle_filled(center, port_radius, color);
    };

    for input_idx in 0..func.inputs.len() {
        let center = layout.input_center(input_idx, row_height);
        draw_port(
            center,
            PortKind::Input,
            input_idx,
            ctx.style.input_port_color,
            ctx.style.input_hover_color,
        );
    }

    for output_idx in 0..func.outputs.len() {
        let center = layout.output_center(output_idx, row_height);
        draw_port(
            center,
            PortKind::Output,
            output_idx,
            ctx.style.output_port_color,
            ctx.style.output_hover_color,
        );
    }
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
