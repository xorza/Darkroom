use crate::common::font::ScaledFontId;
use eframe::egui;
use graph::data::StaticValue;
use graph::graph::{Binding, NodeId};
use graph::prelude::{Func, FuncBehavior, FuncLib, NodeBehavior};
use std::collections::HashMap;
use std::collections::HashSet;

use crate::{
    gui::{graph::GraphUiAction, render::RenderContext},
    model,
};

#[derive(Debug, Default)]
pub struct NodeInteraction {
    pub selection_request: Option<NodeId>,
    pub remove_request: Option<NodeId>,
    pub actions: Vec<(NodeId, GraphUiAction)>,
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

pub fn node_rect_for_graph(
    origin: egui::Pos2,
    view_node: &model::ViewNode,
    input_count: usize,
    output_count: usize,
    scale: f32,
    layout: &NodeLayout,
    node_width: f32,
) -> egui::Rect {
    let node_size = node_size(input_count, output_count, layout, node_width);
    egui::Rect::from_min_size(origin + view_node.pos.to_vec2() * scale, node_size)
}

pub fn render_node_bodies(
    ctx: &RenderContext,
    view_graph: &mut model::ViewGraph,
    func_lib: &FuncLib,
) -> NodeInteraction {
    let visuals = ctx.ui().visuals();
    let node_fill = ctx.style.node_fill;
    let node_stroke = ctx.style.node_stroke;
    let selected_stroke = ctx.style.selected_stroke;
    let mut interaction = NodeInteraction::default();

    for node_view in &mut view_graph.view_nodes {
        let node = view_graph.graph.by_id_mut(&node_view.id).unwrap();
        let func = func_lib.by_id(&node.func_id).unwrap();

        let input_count = func.inputs.len();
        let output_count = func.outputs.len();
        let node_width = ctx.node_width(node.id);
        let node_size = node_size(input_count, output_count, &ctx.layout, node_width);
        let node_rect =
            egui::Rect::from_min_size(ctx.origin + node_view.pos.to_vec2() * ctx.scale, node_size);
        let header_rect = egui::Rect::from_min_size(
            node_rect.min,
            egui::vec2(node_size.x, ctx.layout.header_height),
        );
        let cache_rect = egui::Rect::from_min_size(
            node_rect.min + egui::vec2(0.0, ctx.layout.header_height),
            egui::vec2(node_size.x, ctx.layout.cache_height),
        );
        let button_size = (ctx.layout.header_height - ctx.layout.padding)
            .max(12.0 * ctx.scale)
            .min(ctx.layout.header_height);
        let button_pos = egui::pos2(
            node_rect.max.x - ctx.layout.padding - button_size,
            node_rect.min.y + (ctx.layout.header_height - button_size) * 0.5,
        );
        let close_rect =
            egui::Rect::from_min_size(button_pos, egui::vec2(button_size, button_size));
        let mut header_drag_right = close_rect.min.x - ctx.layout.padding;
        let dot_radius = ctx.scale * ctx.style.status_dot_radius;
        let mut dot_centers = Vec::new();
        let has_terminal = node.terminal;
        let has_impure = func.behavior == FuncBehavior::Impure;
        if has_terminal || has_impure {
            let dot_diameter = dot_radius * 2.0;
            let dot_gap = ctx.scale * ctx.style.status_item_gap;
            let mut dot_x = close_rect.min.x - ctx.layout.padding - dot_radius;
            if has_terminal {
                dot_centers.push((dot_x, "terminal", visuals.selection.stroke.color));
                dot_x -= dot_diameter + dot_gap;
            }
            if has_impure {
                dot_centers.push((dot_x, "impure", egui::Color32::from_rgb(255, 150, 70)));
                dot_x -= dot_diameter + dot_gap;
            }
            header_drag_right = dot_x + dot_gap - ctx.layout.padding;
        }
        let header_drag_rect = egui::Rect::from_min_max(
            header_rect.min,
            egui::pos2(header_drag_right, header_rect.max.y),
        );
        let cache_button_height = if ctx.layout.cache_height > 0.0 {
            let vertical_padding = ctx.layout.padding * ctx.style.cache_button_vertical_pad_factor;
            (ctx.layout.cache_height - vertical_padding * 2.0)
                .max(10.0 * ctx.scale)
                .min(ctx.layout.cache_height)
        } else {
            0.0
        };
        let cache_button_padding = ctx.layout.padding * ctx.style.cache_button_text_pad_factor;

        let cache_text_width = if ctx.layout.cache_height > 0.0 {
            let cached_width = text_width(
                ctx.painter(),
                &ctx.style.body_font.scaled(ctx.scale),
                "cached",
                ctx.style.text_color,
            );
            let cache_width = text_width(
                ctx.painter(),
                &ctx.style.body_font.scaled(ctx.scale),
                "cache",
                ctx.style.text_color,
            );
            cached_width.max(cache_width)
        } else {
            0.0
        };
        let cache_button_width = (cache_button_height * ctx.style.cache_button_width_factor)
            .max(cache_button_height)
            .max(cache_text_width + cache_button_padding * 2.0);

        let cache_button_pos = egui::pos2(
            cache_rect.min.x + ctx.layout.padding,
            cache_rect.min.y + (ctx.layout.cache_height - cache_button_height) * 0.5,
        );
        let cache_button_rect = egui::Rect::from_min_size(
            cache_button_pos,
            egui::vec2(cache_button_width, cache_button_height),
        );
        let node_id = ctx.ui().make_persistent_id(("node_body", node.id));
        let body_response = ctx.ui().interact(node_rect, node_id, egui::Sense::click());

        let close_id = ctx.ui().make_persistent_id(("node_close", node.id));
        let remove_response = ctx
            .ui()
            .interact(close_rect, close_id, egui::Sense::click());
        let cache_enabled = ctx.layout.cache_height > 0.0 && !node.terminal;
        let cache_id = ctx.ui().make_persistent_id(("node_cache", node.id));
        let cache_response = ctx.ui().interact(
            cache_button_rect,
            cache_id,
            if cache_enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );

        let header_id = ctx.ui().make_persistent_id(("node_header", node.id));
        let response = ctx
            .ui()
            .interact(header_drag_rect, header_id, egui::Sense::drag());

        if response.dragged() {
            node_view.pos += response.drag_delta() / ctx.scale;
        }

        if cache_enabled && cache_response.clicked() {
            node.behavior = if node.behavior == NodeBehavior::Once {
                NodeBehavior::AsFunction
            } else {
                NodeBehavior::Once
            };
            interaction
                .actions
                .push((node_view.id, GraphUiAction::CacheToggled));
        }

        if remove_response.hovered() {
            remove_response.show_tooltip_text("Remove node");
        }

        if remove_response.clicked() {
            interaction.remove_request = Some(node_view.id);
            interaction
                .actions
                .push((node_view.id, GraphUiAction::NodeRemoved));
            continue;
        }

        if response.clicked() || response.dragged() || body_response.clicked() {
            interaction.selection_request = Some(node_view.id);
        }

        let selected_id = interaction
            .selection_request
            .or(view_graph.selected_node_id);
        let is_selected = selected_id.is_some_and(|id| id == node_view.id);

        ctx.painter().rect(
            node_rect,
            ctx.layout.corner_radius,
            node_fill,
            if is_selected {
                selected_stroke
            } else {
                node_stroke
            },
            egui::StrokeKind::Inside,
        );

        if ctx.layout.cache_height > 0.0 {
            let button_fill = if !cache_enabled {
                visuals.widgets.noninteractive.bg_fill
            } else if node.behavior == NodeBehavior::Once {
                ctx.style.cache_active_color
            } else if cache_response.is_pointer_button_down_on() {
                visuals.widgets.active.bg_fill
            } else if cache_response.hovered() {
                visuals.widgets.hovered.bg_fill
            } else {
                visuals.widgets.inactive.bg_fill
            };
            let button_stroke = visuals.widgets.inactive.bg_stroke;
            ctx.painter().rect(
                cache_button_rect,
                ctx.layout.corner_radius * 0.5,
                button_fill,
                button_stroke,
                egui::StrokeKind::Inside,
            );

            let button_text = "cache";
            let button_text_color = if !cache_enabled {
                visuals.widgets.noninteractive.fg_stroke.color
            } else if node.behavior == NodeBehavior::Once {
                ctx.style.cache_checked_text_color
            } else {
                visuals.text_color()
            };
            ctx.painter().text(
                cache_button_rect.center(),
                egui::Align2::CENTER_CENTER,
                button_text,
                ctx.style.body_font.scaled(ctx.scale),
                button_text_color,
            );
        }

        let dot_center_y = header_rect.center().y;
        for (index, (center_x, tooltip, color)) in dot_centers.iter().enumerate() {
            let dot_center = egui::pos2(*center_x, dot_center_y);
            ctx.painter().circle_filled(dot_center, dot_radius, *color);
            let dot_rect = egui::Rect::from_center_size(
                dot_center,
                egui::vec2(dot_radius * 2.0, dot_radius * 2.0),
            );
            let dot_id = ctx.ui().make_persistent_id(("node_status", node.id, index));
            let dot_response = ctx.ui().interact(dot_rect, dot_id, egui::Sense::hover());
            if dot_response.hovered() {
                dot_response.show_tooltip_text(*tooltip);
            }
        }

        let close_fill = if remove_response.is_pointer_button_down_on() {
            visuals.widgets.active.bg_fill
        } else if remove_response.hovered() {
            visuals.widgets.hovered.bg_fill
        } else {
            visuals.widgets.inactive.bg_fill
        };
        let close_stroke = visuals.widgets.inactive.bg_stroke;
        ctx.painter().rect(
            close_rect,
            ctx.layout.corner_radius * 0.6,
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
        let close_color = visuals.text_color();
        let close_stroke = egui::Stroke::new(1.4 * ctx.scale, close_color);
        ctx.painter().line_segment([a, b], close_stroke);
        ctx.painter().line_segment([c, d], close_stroke);

        render_node_ports(ctx, node_view, input_count, output_count, node_width);
        render_node_const_bindings(ctx, node_view, node, func, input_count);
        render_node_labels(ctx, &node.name, func, node_rect, node_width);
    }

    interaction
}

fn render_node_ports(
    ctx: &RenderContext,
    view_node: &model::ViewNode,
    input_count: usize,
    output_count: usize,
    node_width: f32,
) {
    for index in 0..input_count {
        let center = node_input_pos(
            ctx.origin,
            view_node,
            index,
            input_count,
            &ctx.layout,
            ctx.scale,
        );
        let port_rect = egui::Rect::from_center_size(
            center,
            egui::vec2(
                ctx.scale * ctx.style.port_radius * 2.0,
                ctx.scale * ctx.style.port_radius * 2.0,
            ),
        );
        let color = if ctx.ui().rect_contains_pointer(port_rect) {
            ctx.style.input_hover_color
        } else {
            ctx.style.input_port_color
        };
        ctx.painter()
            .circle_filled(center, ctx.scale * ctx.style.port_radius, color);
    }

    for index in 0..output_count {
        let center = node_output_pos(
            ctx.origin,
            view_node,
            index,
            &ctx.layout,
            ctx.scale,
            node_width,
        );
        let port_rect = egui::Rect::from_center_size(
            center,
            egui::vec2(
                ctx.scale * ctx.style.port_radius * 2.0,
                ctx.scale * ctx.style.port_radius * 2.0,
            ),
        );
        let color = if ctx.ui().rect_contains_pointer(port_rect) {
            ctx.style.output_hover_color
        } else {
            ctx.style.output_port_color
        };
        ctx.painter()
            .circle_filled(center, ctx.scale * ctx.style.port_radius, color);
    }
}

fn render_node_const_bindings(
    ctx: &RenderContext,
    view_node: &model::ViewNode,
    node: &graph::graph::Node,
    func: &Func,
    input_count: usize,
) {
    let badge_padding = 4.0 * ctx.scale;
    let badge_height = (ctx.layout.row_height * 1.2).max(10.0 * ctx.scale);
    let badge_radius = 6.0 * ctx.scale;
    let badge_gap = 6.0 * ctx.scale;

    for (index, input) in node.inputs.iter().enumerate() {
        if index >= input_count {
            break;
        }
        let Binding::Const(value) = &input.binding else {
            continue;
        };

        let label = static_value_label(value);
        let label_width = text_width(
            ctx.painter(),
            &ctx.style.body_font,
            &label,
            ctx.style.text_color,
        );
        let badge_width = label_width + badge_padding * 2.0;
        let center = node_input_pos(
            ctx.origin,
            view_node,
            index,
            func.inputs.len(),
            &ctx.layout,
            ctx.scale,
        );
        let badge_right = center.x - ctx.scale * ctx.style.port_radius - badge_gap;
        let badge_rect = egui::Rect::from_min_max(
            egui::pos2(badge_right - badge_width, center.y - badge_height * 0.5),
            egui::pos2(badge_right, center.y + badge_height * 0.5),
        );

        let link_start = egui::pos2(badge_rect.max.x, center.y);
        let link_end = egui::pos2(center.x - ctx.scale * ctx.style.port_radius * 0.6, center.y);
        ctx.painter()
            .line_segment([link_start, link_end], ctx.style.connection_stroke);
        ctx.painter().rect(
            badge_rect,
            badge_radius,
            ctx.style.node_fill,
            ctx.style.const_stroke,
            egui::StrokeKind::Inside,
        );
        ctx.painter().text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            ctx.style.body_font.scaled(ctx.scale),
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

fn render_node_labels(
    ctx: &RenderContext,
    node_name: &str,
    func: &Func,
    node_rect: egui::Rect,
    node_width: f32,
) {
    let header_text_offset = ctx.scale * ctx.style.header_text_offset;

    ctx.painter().text(
        node_rect.min + egui::vec2(ctx.layout.padding, header_text_offset),
        egui::Align2::LEFT_TOP,
        node_name,
        ctx.style.heading_font.scaled(ctx.scale),
        ctx.style.text_color,
    );

    for (index, input) in func.inputs.iter().enumerate() {
        let text_pos = node_rect.min
            + egui::vec2(
                ctx.layout.padding,
                ctx.layout.header_height
                    + ctx.layout.cache_height
                    + ctx.layout.padding
                    + ctx.layout.row_height * index as f32,
            );
        ctx.painter().text(
            text_pos,
            egui::Align2::LEFT_TOP,
            &input.name,
            ctx.style.body_font.scaled(ctx.scale),
            ctx.style.text_color,
        );
    }

    for (index, output) in func.outputs.iter().enumerate() {
        let text_pos = node_rect.min
            + egui::vec2(
                node_width - ctx.layout.padding,
                ctx.layout.header_height
                    + ctx.layout.cache_height
                    + ctx.layout.padding
                    + ctx.layout.row_height * index as f32,
            );
        ctx.painter().text(
            text_pos,
            egui::Align2::RIGHT_TOP,
            &output.name,
            ctx.style.body_font.scaled(ctx.scale),
            ctx.style.text_color,
        );
    }
}

fn node_size(
    input_count: usize,
    output_count: usize,
    layout: &NodeLayout,
    node_width: f32,
) -> egui::Vec2 {
    let row_count = input_count.max(output_count).max(1);
    let height = layout.header_height
        + layout.cache_height
        + layout.padding
        + layout.row_height * row_count as f32
        + layout.padding;
    egui::vec2(node_width, height)
}

pub(crate) fn node_input_pos(
    origin: egui::Pos2,
    view_node: &model::ViewNode,
    index: usize,
    input_count: usize,
    layout: &NodeLayout,
    scale: f32,
) -> egui::Pos2 {
    assert!(
        index < input_count,
        "input index must be within node inputs"
    );
    assert!(scale > 0.0, "graph scale must be positive");
    let y = origin.y
        + view_node.pos.y * scale
        + layout.header_height
        + layout.cache_height
        + layout.padding
        + layout.row_height * index as f32
        + layout.row_height * 0.5;
    egui::pos2(origin.x + view_node.pos.x * scale, y)
}

pub(crate) fn node_output_pos(
    origin: egui::Pos2,
    view_node: &model::ViewNode,
    index: usize,
    layout: &NodeLayout,
    scale: f32,
    node_width: f32,
) -> egui::Pos2 {
    let y = origin.y
        + view_node.pos.y * scale
        + layout.header_height
        + layout.cache_height
        + layout.padding
        + layout.row_height * index as f32
        + layout.row_height * 0.5;
    egui::pos2(origin.x + view_node.pos.x * scale + node_width, y)
}

pub(crate) fn bezier_control_offset(start: egui::Pos2, end: egui::Pos2, scale: f32) -> f32 {
    let dx = (end.x - start.x).abs();
    (dx * 0.5).max(40.0 * scale)
}

#[derive(Debug)]
pub(crate) struct NodeWidthContext<'a> {
    pub layout: &'a NodeLayout,
    pub style: &'a crate::gui::style::Style,
    pub scale: f32,
}

pub(crate) fn compute_node_widths(
    painter: &egui::Painter,
    view_graph: &model::ViewGraph,
    func_lib: &FuncLib,
    ctx: &NodeWidthContext<'_>,
) -> HashMap<NodeId, f32> {
    let scale_guess = ctx.layout.row_height / 18.0;
    let mut widths = HashMap::with_capacity(view_graph.view_nodes.len());

    for node_view in &view_graph.view_nodes {
        let node = view_graph.graph.by_id(&node_view.id).unwrap();
        let func = func_lib.by_id(&node.func_id).unwrap();
        let header_width = text_width(
            painter,
            &ctx.style.heading_font.scaled(ctx.scale),
            &node.name,
            ctx.style.text_color,
        ) + ctx.layout.padding * 2.0;
        let vertical_padding = ctx.layout.padding * ctx.style.cache_button_vertical_pad_factor;
        let cache_button_height = (ctx.layout.cache_height - vertical_padding * 2.0)
            .max(10.0 * scale_guess)
            .min(ctx.layout.cache_height);
        let cache_text_width = text_width(
            painter,
            &ctx.style.body_font.scaled(ctx.scale),
            "cached",
            ctx.style.text_color,
        )
        .max(text_width(
            painter,
            &ctx.style.body_font.scaled(ctx.scale),
            "cache",
            ctx.style.text_color,
        ));
        let cache_button_width = (cache_button_height * ctx.style.cache_button_width_factor)
            .max(cache_button_height)
            .max(
                cache_text_width
                    + ctx.layout.padding * ctx.style.cache_button_text_pad_factor * 2.0,
            );
        let cache_row_width = if ctx.layout.cache_height > 0.0 {
            ctx.layout.padding + cache_button_width + ctx.layout.padding
        } else {
            0.0
        };
        let status_row_width = {
            let dot_diameter = ctx.style.status_dot_radius * 2.0;
            let count = 2usize;
            let gaps = (count - 1) as f32;
            let total = count as f32 * dot_diameter + gaps * ctx.style.status_item_gap;
            ctx.layout.padding + total + ctx.layout.padding
        };

        let input_widths: Vec<f32> = func
            .inputs
            .iter()
            .map(|input| {
                text_width(
                    painter,
                    &ctx.style.body_font.scaled(ctx.scale),
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
                    painter,
                    &ctx.style.body_font.scaled(ctx.scale),
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
            let mut row_width = ctx.layout.padding * 2.0 + left + right;
            if left > 0.0 && right > 0.0 {
                row_width += inter_side_padding;
            }
            max_row_width = max_row_width.max(row_width);
        }

        let computed = ctx.layout.node_width.max(
            header_width
                .max(max_row_width)
                .max(cache_row_width)
                .max(status_row_width),
        );
        widths.insert(node.id, computed);
    }

    widths
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
