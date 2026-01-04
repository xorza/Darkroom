use crate::common::font::ScaledFontId;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use eframe::egui;
use egui::{PointerButton, Pos2, Rect, Sense};
use graph::data::StaticValue;
use graph::graph::{Binding, NodeId};
use graph::prelude::{FuncBehavior, NodeBehavior};
use hashbrown::HashMap;

use crate::{
    gui::{graph_ctx::GraphContext, graph_ui::GraphUiAction, graph_ui::GraphUiInteraction},
    model,
};

#[derive(Debug, Clone)]
pub enum PortDragInfo {
    None,
    Hover(PortInfo),
    DragStart(PortInfo),
    DragStop,
}

#[derive(Debug)]
pub struct NodeLayout {
    pub node_width: f32,
    pub header_height: f32,
    pub cache_height: f32,
    pub row_height: f32,
    pub padding: f32,
    pub corner_radius: f32,
}

impl Default for NodeLayout {
    fn default() -> Self {
        Self {
            node_width: 180.0,
            header_height: 22.0,
            cache_height: 20.0,
            row_height: 18.0,
            padding: 8.0,
            corner_radius: 6.0,
        }
    }
}

#[derive(Debug, Default)]
pub struct NodeUi {}

impl NodeLayout {
    pub(crate) fn scaled(&self, scale: f32) -> Self {
        Self {
            node_width: self.node_width * scale,
            header_height: self.header_height * scale,
            cache_height: self.cache_height * scale,
            row_height: self.row_height * scale,
            padding: self.padding * scale,
            corner_radius: self.corner_radius * scale,
        }
    }
}

impl NodeUi {
    pub fn process_input(
        &mut self,
        ctx: &mut GraphContext,
        graph_layout: &mut GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
    ) {
        for view_node_idx in 0..ctx.view_graph.view_nodes.len() {
            let view_node_id = ctx.view_graph.view_nodes[view_node_idx].id;
            let node_rect = graph_layout.node_rect(&view_node_id);

            let node_id = ctx.ui.make_persistent_id(("node_body", view_node_id));
            let body_response = ctx.ui.interact(
                node_rect,
                node_id,
                egui::Sense::click() | egui::Sense::hover() | egui::Sense::drag(),
            );

            let dragged = body_response.dragged_by(PointerButton::Middle)
                || body_response.dragged_by(PointerButton::Primary);
            if dragged {
                ctx.view_graph.view_nodes[view_node_idx].pos +=
                    body_response.drag_delta() / ctx.view_graph.scale;

                let new_rect = compute_node_rect(
                    ctx,
                    &view_node_id,
                    &graph_layout.node_layout,
                    graph_layout.origin,
                );
                graph_layout.update_node_rect_position(&view_node_id, new_rect);
            }
            if dragged || body_response.clicked() {
                ui_interaction
                    .actions
                    .push((view_node_id, GraphUiAction::NodeSelected));
                ctx.view_graph.select_node(&view_node_id);
            }
        }
    }

    pub fn render_nodes(
        &mut self,
        ctx: &mut GraphContext,
        graph_layout: &mut GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
    ) -> PortDragInfo {
        let mut drag_port_info: PortDragInfo = PortDragInfo::None;

        for view_node_idx in 0..ctx.view_graph.view_nodes.len() {
            let view_node_id = ctx.view_graph.view_nodes[view_node_idx].id;
            let node_rect = graph_layout.node_rect(&view_node_id);

            let node = ctx.view_graph.graph.by_id_mut(&view_node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();

            let input_count = func.inputs.len();
            let output_count = func.outputs.len();

            let header_rect = egui::Rect::from_min_size(
                node_rect.min,
                egui::vec2(node_rect.size().x, graph_layout.node_layout.header_height),
            );
            let cache_rect = egui::Rect::from_min_size(
                node_rect.min + egui::vec2(0.0, graph_layout.node_layout.header_height),
                egui::vec2(node_rect.size().x, graph_layout.node_layout.cache_height),
            );
            let button_size = (graph_layout.node_layout.header_height
                - graph_layout.node_layout.padding)
                .max(12.0 * ctx.view_graph.scale)
                .min(graph_layout.node_layout.header_height);
            let button_pos = egui::pos2(
                node_rect.max.x - graph_layout.node_layout.padding - button_size,
                node_rect.min.y + (graph_layout.node_layout.header_height - button_size) * 0.5,
            );
            let close_rect =
                egui::Rect::from_min_size(button_pos, egui::vec2(button_size, button_size));
            let dot_radius = ctx.view_graph.scale * ctx.style.status_dot_radius;
            let mut dot_centers = Vec::new();
            let has_terminal = node.terminal;
            let has_impure = func.behavior == FuncBehavior::Impure;
            if has_terminal || has_impure {
                let dot_diameter = dot_radius * 2.0;
                let dot_gap = ctx.view_graph.scale * ctx.style.status_item_gap;
                let mut dot_x = close_rect.min.x - graph_layout.node_layout.padding - dot_radius;
                if has_terminal {
                    dot_centers.push((dot_x, "terminal", ctx.style.status_terminal_color));
                    dot_x -= dot_diameter + dot_gap;
                }
                if has_impure {
                    dot_centers.push((dot_x, "impure", ctx.style.status_impure_color));
                }
            }
            let cache_button_height = if graph_layout.node_layout.cache_height > 0.0 {
                let vertical_padding =
                    graph_layout.node_layout.padding * ctx.style.cache_button_vertical_pad_factor;
                (graph_layout.node_layout.cache_height - vertical_padding * 2.0)
                    .max(10.0 * ctx.view_graph.scale)
                    .min(graph_layout.node_layout.cache_height)
            } else {
                0.0
            };
            let cache_button_padding =
                graph_layout.node_layout.padding * ctx.style.cache_button_text_pad_factor;

            let cache_text_width = text_width(
                &ctx.painter,
                &ctx.style.body_font.scaled(ctx.view_graph.scale),
                "cache",
                ctx.style.text_color,
            );
            let cache_button_width = (cache_button_height * ctx.style.cache_button_width_factor)
                .max(cache_button_height)
                .max(cache_text_width + cache_button_padding * 2.0);

            let cache_button_pos = egui::pos2(
                cache_rect.min.x + graph_layout.node_layout.padding,
                cache_rect.min.y
                    + (graph_layout.node_layout.cache_height - cache_button_height) * 0.5,
            );
            let cache_button_rect = egui::Rect::from_min_size(
                cache_button_pos,
                egui::vec2(cache_button_width, cache_button_height),
            );

            let close_id = ctx.ui.make_persistent_id(("node_close", node.id));
            let remove_response = ctx.ui.interact(close_rect, close_id, egui::Sense::click());
            let cache_enabled = graph_layout.node_layout.cache_height > 0.0 && !node.terminal;
            let cache_id = ctx.ui.make_persistent_id(("node_cache", node.id));
            let cache_response = ctx.ui.interact(
                cache_button_rect,
                cache_id,
                if cache_enabled {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                },
            );

            if cache_enabled && cache_response.clicked() {
                node.behavior = if node.behavior == NodeBehavior::Once {
                    NodeBehavior::AsFunction
                } else {
                    NodeBehavior::Once
                };
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
                ctx.view_graph.remove_node(&view_node_id);

                continue;
            }

            let is_selected = ctx
                .view_graph
                .selected_node_id
                .is_some_and(|id| id == view_node_id);

            ctx.painter.rect(
                node_rect,
                graph_layout.node_layout.corner_radius,
                ctx.style.node_fill,
                if is_selected {
                    ctx.style.selected_stroke
                } else {
                    ctx.style.node_stroke
                },
                egui::StrokeKind::Inside,
            );

            if graph_layout.node_layout.cache_height > 0.0 {
                let button_fill = if !cache_enabled {
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
                    cache_button_rect,
                    graph_layout.node_layout.corner_radius * 0.5,
                    button_fill,
                    button_stroke,
                    egui::StrokeKind::Inside,
                );

                let button_text_color = if !cache_enabled {
                    ctx.style.widget_noninteractive_text_color
                } else if node.behavior == NodeBehavior::Once {
                    ctx.style.cache_checked_text_color
                } else {
                    ctx.style.widget_text_color
                };
                ctx.painter.text(
                    cache_button_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "cache",
                    ctx.style.body_font.scaled(ctx.view_graph.scale),
                    button_text_color,
                );
            }

            let dot_center_y = header_rect.center().y;
            for (index, (center_x, tooltip, color)) in dot_centers.iter().enumerate() {
                let dot_center = egui::pos2(*center_x, dot_center_y);
                ctx.painter.circle_filled(dot_center, dot_radius, *color);
                let dot_rect = egui::Rect::from_center_size(
                    dot_center,
                    egui::vec2(dot_radius * 2.0, dot_radius * 2.0),
                );
                let dot_id = ctx.ui.make_persistent_id(("node_status", node.id, index));
                let dot_response = ctx.ui.interact(dot_rect, dot_id, egui::Sense::hover());
                if dot_response.hovered() {
                    dot_response.show_tooltip_text(*tooltip);
                }
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
                close_rect,
                graph_layout.node_layout.corner_radius * 0.6,
                close_fill,
                close_stroke,
                egui::StrokeKind::Inside,
            );
            let close_margin = button_size * 0.3;
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
            let close_stroke = egui::Stroke::new(1.4 * ctx.view_graph.scale, close_color);
            ctx.painter.line_segment([a, b], close_stroke);
            ctx.painter.line_segment([c, d], close_stroke);

            let node_drag_port_result = render_node_ports(
                ctx,
                graph_layout,
                view_node_idx,
                input_count,
                output_count,
                node_rect.width(),
            );
            if node_drag_port_result.prio() > drag_port_info.prio() {
                drag_port_info = node_drag_port_result;
            }

            render_node_const_bindings(ctx, graph_layout, view_node_idx);
            render_node_labels(ctx, graph_layout, view_node_idx);
        }

        drag_port_info
    }
}

fn render_node_ports(
    ctx: &GraphContext,
    graph_layout: &GraphLayout,
    view_node_idx: usize,
    input_count: usize,
    output_count: usize,
    node_width: f32,
) -> PortDragInfo {
    let view_node = &ctx.view_graph.view_nodes[view_node_idx];
    let mut port_drag_info: PortDragInfo = PortDragInfo::None;

    let port_radius = ctx.view_graph.scale * ctx.style.port_radius;
    let port_rect_size = egui::vec2(port_radius * 2.0, port_radius * 2.0);

    let mut handle_port = |center: Pos2,
                           kind: PortKind,
                           idx: usize,
                           base_color: egui::Color32,
                           hover_color: egui::Color32| {
        let port_rect = egui::Rect::from_center_size(center, port_rect_size);
        let graph_bg_id = ctx
            .ui
            .make_persistent_id(("node_port", kind, view_node_idx, idx));
        let response = ctx
            .ui
            .interact(port_rect, graph_bg_id, Sense::hover() | Sense::drag());
        let is_hovered = ctx.ui.rect_contains_pointer(port_rect);

        let color = if is_hovered { hover_color } else { base_color };
        ctx.painter.circle_filled(center, port_radius, color);

        let port_info = PortInfo {
            port: PortRef {
                node_id: view_node.id,
                idx,
                kind,
            },
            center,
        };
        let drag_info = if response.drag_started_by(PointerButton::Primary) {
            PortDragInfo::DragStart(port_info)
        } else if response.drag_stopped_by(PointerButton::Primary) {
            PortDragInfo::DragStop
        } else if is_hovered {
            PortDragInfo::Hover(port_info)
        } else {
            PortDragInfo::None
        };

        if drag_info.prio() > port_drag_info.prio() {
            port_drag_info = drag_info;
        }
    };

    for input_idx in 0..input_count {
        let center = node_input_pos(
            graph_layout.origin,
            view_node,
            input_idx,
            &graph_layout.node_layout,
            ctx.view_graph.scale,
        );
        handle_port(
            center,
            PortKind::Input,
            input_idx,
            ctx.style.input_port_color,
            ctx.style.input_hover_color,
        );
    }

    for output_idx in 0..output_count {
        let center = node_output_pos(
            graph_layout.origin,
            view_node,
            output_idx,
            &graph_layout.node_layout,
            ctx.view_graph.scale,
            node_width,
        );
        handle_port(
            center,
            PortKind::Output,
            output_idx,
            ctx.style.output_port_color,
            ctx.style.output_hover_color,
        );
    }

    port_drag_info
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
}

fn render_node_const_bindings(
    ctx: &mut GraphContext,
    graph_layout: &GraphLayout,
    view_node_idx: usize,
) {
    let view_node = &mut ctx.view_graph.view_nodes[view_node_idx];
    let node = ctx.view_graph.graph.by_id_mut(&view_node.id).unwrap();
    let func = ctx.func_lib.by_id(&node.func_id).unwrap();

    let badge_padding = 4.0 * ctx.view_graph.scale;
    let badge_height = (graph_layout.node_layout.row_height * 1.2).max(10.0 * ctx.view_graph.scale);
    let badge_radius = 6.0 * ctx.view_graph.scale;
    let badge_gap = 6.0 * ctx.view_graph.scale;

    for (index, input) in node.inputs.iter().enumerate() {
        if index >= func.inputs.len() {
            break;
        }
        let Binding::Const(value) = &input.binding else {
            continue;
        };

        let label = static_value_label(value);
        let label_width = text_width(
            &ctx.painter,
            &ctx.style.body_font,
            &label,
            ctx.style.text_color,
        );
        let badge_width = label_width + badge_padding * 2.0;
        let center = node_input_pos(
            graph_layout.origin,
            &ctx.view_graph.view_nodes[view_node_idx],
            index,
            &graph_layout.node_layout,
            ctx.view_graph.scale,
        );
        let badge_right = center.x - ctx.view_graph.scale * ctx.style.port_radius - badge_gap;
        let badge_rect = egui::Rect::from_min_max(
            egui::pos2(badge_right - badge_width, center.y - badge_height * 0.5),
            egui::pos2(badge_right, center.y + badge_height * 0.5),
        );

        let link_start = egui::pos2(badge_rect.max.x, center.y);
        let link_end = egui::pos2(
            center.x - ctx.view_graph.scale * ctx.style.port_radius * 0.6,
            center.y,
        );
        ctx.painter
            .line_segment([link_start, link_end], ctx.style.connection_stroke);
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
            ctx.style.body_font.scaled(ctx.view_graph.scale),
            ctx.style.text_color,
        );
    }
}

fn static_value_label(value: &StaticValue) -> String {
    match value {
        StaticValue::Null => "null".to_string(),
        StaticValue::Float(value) => {
            if value.fract() == 0.0 {
                format!("{:.0}", value)
            } else {
                format!("{:.3}", value)
            }
        }
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

fn render_node_labels(ctx: &mut GraphContext, graph_layout: &GraphLayout, view_node_idx: usize) {
    let view_node = &mut ctx.view_graph.view_nodes[view_node_idx];
    let node = ctx.view_graph.graph.by_id_mut(&view_node.id).unwrap();
    let func = ctx.func_lib.by_id(&node.func_id).unwrap();

    let header_text_offset = ctx.view_graph.scale * ctx.style.header_text_offset;

    let node_rect = graph_layout.node_rect(&view_node.id);

    ctx.painter.text(
        node_rect.min + egui::vec2(graph_layout.node_layout.padding, header_text_offset),
        egui::Align2::LEFT_TOP,
        &mut node.name,
        ctx.style.heading_font.scaled(ctx.view_graph.scale),
        ctx.style.text_color,
    );

    for (index, input) in func.inputs.iter().enumerate() {
        let text_pos = node_rect.min
            + egui::vec2(
                graph_layout.node_layout.padding,
                graph_layout.node_layout.header_height
                    + graph_layout.node_layout.cache_height
                    + graph_layout.node_layout.padding
                    + graph_layout.node_layout.row_height * index as f32,
            );
        ctx.painter.text(
            text_pos,
            egui::Align2::LEFT_TOP,
            &input.name,
            ctx.style.body_font.scaled(ctx.view_graph.scale),
            ctx.style.text_color,
        );
    }

    for (index, output) in func.outputs.iter().enumerate() {
        let text_pos = node_rect.min
            + egui::vec2(
                node_rect.width() - graph_layout.node_layout.padding,
                graph_layout.node_layout.header_height
                    + graph_layout.node_layout.cache_height
                    + graph_layout.node_layout.padding
                    + graph_layout.node_layout.row_height * index as f32,
            );
        ctx.painter.text(
            text_pos,
            egui::Align2::RIGHT_TOP,
            &output.name,
            ctx.style.body_font.scaled(ctx.view_graph.scale),
            ctx.style.text_color,
        );
    }
}

pub(crate) fn node_input_pos(
    origin: egui::Pos2,
    view_node: &model::ViewNode,
    index: usize,
    node_layout: &NodeLayout,
    scale: f32,
) -> egui::Pos2 {
    let y = origin.y
        + view_node.pos.y * scale
        + node_layout.header_height
        + node_layout.cache_height
        + node_layout.padding
        + node_layout.row_height * index as f32
        + node_layout.row_height * 0.5;
    egui::pos2(origin.x + view_node.pos.x * scale, y)
}

pub(crate) fn node_output_pos(
    origin: egui::Pos2,
    view_node: &model::ViewNode,
    index: usize,
    node_layout: &NodeLayout,
    scale: f32,
    node_width: f32,
) -> egui::Pos2 {
    let y = origin.y
        + view_node.pos.y * scale
        + node_layout.header_height
        + node_layout.cache_height
        + node_layout.padding
        + node_layout.row_height * index as f32
        + node_layout.row_height * 0.5;
    egui::pos2(origin.x + view_node.pos.x * scale + node_width, y)
}

pub(crate) fn bezier_control_offset(start: egui::Pos2, end: egui::Pos2, scale: f32) -> f32 {
    let dx = (end.x - start.x).abs();
    (dx * 0.5).max(40.0 * scale)
}

pub(crate) fn compute_node_rects(
    ctx: &GraphContext,
    node_layout: &NodeLayout,
    origin: Pos2,
    node_rects: &mut HashMap<NodeId, Rect>,
) {
    node_rects.clear();

    for view_node in ctx.view_graph.view_nodes.iter() {
        let rect = compute_node_rect(ctx, &view_node.id, node_layout, origin);
        node_rects.insert(view_node.id, rect);
    }
}

fn compute_node_rect(
    ctx: &GraphContext,
    view_node_id: &NodeId,
    node_layout: &NodeLayout,
    origin: Pos2,
) -> Rect {
    let node = ctx.view_graph.graph.by_id(view_node_id).unwrap();
    let func = ctx.func_lib.by_id(&node.func_id).unwrap();

    let header_width = text_width(
        &ctx.painter,
        &ctx.style.heading_font.scaled(ctx.view_graph.scale),
        &node.name,
        ctx.style.text_color,
    ) + node_layout.padding * 2.0;
    let vertical_padding = node_layout.padding * ctx.style.cache_button_vertical_pad_factor;
    let cache_button_height = (node_layout.cache_height - vertical_padding * 2.0)
        .max(10.0 * ctx.view_graph.scale)
        .min(node_layout.cache_height);
    let cache_text_width = text_width(
        &ctx.painter,
        &ctx.style.body_font.scaled(ctx.view_graph.scale),
        "cache",
        ctx.style.text_color,
    );
    let cache_button_width = (cache_button_height * ctx.style.cache_button_width_factor)
        .max(cache_button_height)
        .max(cache_text_width + node_layout.padding * ctx.style.cache_button_text_pad_factor * 2.0);
    let cache_row_width = if node_layout.cache_height > 0.0 {
        node_layout.padding + cache_button_width + node_layout.padding
    } else {
        0.0
    };
    let status_row_width = {
        let dot_diameter = ctx.style.status_dot_radius * 2.0;
        let count = 2usize;
        let gaps = (count - 1) as f32;
        let total = count as f32 * dot_diameter + gaps * ctx.style.status_item_gap;
        node_layout.padding + total + node_layout.padding
    };

    let input_widths: Vec<f32> = func
        .inputs
        .iter()
        .map(|input| {
            text_width(
                &ctx.painter,
                &ctx.style.body_font.scaled(ctx.view_graph.scale),
                &input.name,
                ctx.style.text_color,
            )
        })
        .collect();
    let output_widths: Vec<f32> = func
        .outputs
        .iter()
        .map(|output| {
            text_width(
                &ctx.painter,
                &ctx.style.body_font.scaled(ctx.view_graph.scale),
                &output.name,
                ctx.style.text_color,
            )
        })
        .collect();

    let row_count = func.inputs.len().max(func.outputs.len()).max(1);
    let mut max_row_width: f32 = 0.0;

    let inter_side_padding = 0.0;
    for row in 0..row_count {
        let left = input_widths.get(row).copied().unwrap_or(0.0);
        let right = output_widths.get(row).copied().unwrap_or(0.0);
        let mut row_width = node_layout.padding * 2.0 + left + right;
        if left > 0.0 && right > 0.0 {
            row_width += inter_side_padding;
        }
        max_row_width = max_row_width.max(row_width);
    }

    let node_width = node_layout
        .node_width
        .max(header_width)
        .max(max_row_width)
        .max(cache_row_width)
        .max(status_row_width);

    let row_count = func.inputs.len().max(func.outputs.len()).max(1);
    let height = node_layout.header_height
        + node_layout.cache_height
        + node_layout.padding
        + node_layout.row_height * row_count as f32
        + node_layout.padding;
    let node_size = egui::vec2(node_width, height);

    let view_node = ctx.view_graph.view_nodes.by_key(view_node_id).unwrap();
    egui::Rect::from_min_size(
        origin + view_node.pos.to_vec2() * ctx.view_graph.scale,
        node_size,
    )
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
