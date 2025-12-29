use eframe::egui;
use graph::prelude::NodeBehavior;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{gui::render::RenderContext, model};

#[derive(Debug, Default)]
pub struct NodeInteraction {
    pub selection_request: Option<Uuid>,
    pub remove_request: Option<Uuid>,
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
    pub(crate) fn assert_valid(&self) {
        assert!(self.node_width > 0.0, "node width must be positive");
        assert!(
            self.header_height >= 0.0,
            "header height must be non-negative"
        );
        assert!(
            self.cache_height >= 0.0,
            "cache height must be non-negative"
        );
        assert!(self.row_height > 0.0, "row height must be positive");
        assert!(self.padding >= 0.0, "padding must be non-negative");
        assert!(
            self.corner_radius >= 0.0,
            "corner radius must be non-negative"
        );
    }

    pub(crate) fn scaled(&self, scale: f32) -> Self {
        assert!(scale > 0.0, "layout scale must be positive");
        assert!(scale.is_finite(), "layout scale must be finite");

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
    node: &model::NodeView,
    scale: f32,
    layout: &NodeLayout,
    node_width: f32,
) -> egui::Rect {
    assert!(scale > 0.0, "graph scale must be positive");
    assert!(scale.is_finite(), "graph scale must be finite");
    layout.assert_valid();
    let node_size = node_size(node, layout, node_width);
    egui::Rect::from_min_size(origin + node.pos.to_vec2() * scale, node_size)
}

pub(crate) fn port_radius_for_scale(scale: f32) -> f32 {
    assert!(scale.is_finite(), "port scale must be finite");
    assert!(scale > 0.0, "port scale must be positive");
    let radius = (5.5 * scale).clamp(3.0, 7.5);
    assert!(radius.is_finite(), "port radius must be finite");
    assert!(radius > 0.0, "port radius must be positive");
    radius
}

pub fn render_node_bodies(ctx: &RenderContext, graph: &mut model::GraphView) -> NodeInteraction {
    let visuals = ctx.ui().visuals();
    let node_fill = ctx.style.node_fill;
    let node_stroke = ctx.style.node_stroke;
    let selected_stroke = ctx.style.selected_stroke;
    let mut interaction = NodeInteraction::default();

    for node in &mut graph.nodes {
        let node_width = ctx.node_width(node.id);
        let node_size = node_size(node, &ctx.layout, node_width);
        let node_rect =
            egui::Rect::from_min_size(ctx.origin + node.pos.to_vec2() * ctx.scale, node_size);
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
        assert!(button_size.is_finite(), "close button size must be finite");
        assert!(button_size > 0.0, "close button size must be positive");
        let button_pos = egui::pos2(
            node_rect.max.x - ctx.layout.padding - button_size,
            node_rect.min.y + (ctx.layout.header_height - button_size) * 0.5,
        );
        let close_rect =
            egui::Rect::from_min_size(button_pos, egui::vec2(button_size, button_size));
        let mut header_drag_right = close_rect.min.x - ctx.layout.padding;
        let dot_radius = ctx.style.status_dot_radius;
        assert!(dot_radius.is_finite(), "status dot radius must be finite");
        assert!(dot_radius >= 0.0, "status dot radius must be non-negative");
        let mut dot_centers = Vec::new();
        if node.behavior == NodeBehavior::Terminal {
            let dot_diameter = dot_radius * 2.0;
            let dot_gap = ctx.style.status_item_gap;
            let mut dot_x = close_rect.min.x - ctx.layout.padding - dot_radius;
            dot_centers.push((dot_x, "terminal", visuals.selection.stroke.color));
            dot_x -= dot_diameter + dot_gap;
            header_drag_right = dot_x + dot_gap - ctx.layout.padding;
        }
        let header_drag_rect = egui::Rect::from_min_max(
            header_rect.min,
            egui::pos2(header_drag_right, header_rect.max.y),
        );
        let cache_button_height = if ctx.layout.cache_height > 0.0 {
            let vertical_padding = ctx.layout.padding * ctx.style.cache_button_vertical_pad_factor;
            let size = (ctx.layout.cache_height - vertical_padding * 2.0)
                .max(10.0 * ctx.scale)
                .min(ctx.layout.cache_height);
            assert!(size.is_finite(), "cache button height must be finite");
            assert!(size > 0.0, "cache button height must be positive");
            size
        } else {
            0.0
        };
        let cache_button_padding = ctx.layout.padding * ctx.style.cache_button_text_pad_factor;
        assert!(
            cache_button_padding.is_finite(),
            "cache button padding must be finite"
        );
        assert!(
            cache_button_padding >= 0.0,
            "cache button padding must be non-negative"
        );
        let cache_text_width = if ctx.layout.cache_height > 0.0 {
            let cached_width = text_width(ctx.painter(), &ctx.body_font, "cached", ctx.text_color);
            let cache_width = text_width(ctx.painter(), &ctx.body_font, "cache", ctx.text_color);
            cached_width.max(cache_width)
        } else {
            0.0
        };
        let cache_button_width = (cache_button_height * ctx.style.cache_button_width_factor)
            .max(cache_button_height)
            .max(cache_text_width + cache_button_padding * 2.0);
        assert!(
            cache_button_width.is_finite(),
            "cache button width must be finite"
        );
        assert!(
            cache_button_width > 0.0,
            "cache button width must be positive"
        );
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
        let close_response = ctx
            .ui()
            .interact(close_rect, close_id, egui::Sense::click());
        let cache_id = ctx.ui().make_persistent_id(("node_cache", node.id));
        let cache_response = ctx
            .ui()
            .interact(cache_button_rect, cache_id, egui::Sense::click());

        let header_id = ctx.ui().make_persistent_id(("node_header", node.id));
        let response = ctx
            .ui()
            .interact(header_drag_rect, header_id, egui::Sense::drag());

        if response.dragged() {
            node.pos += response.drag_delta() / ctx.scale;
        }

        if ctx.layout.cache_height > 0.0 && cache_response.clicked() {
            node.behavior = if node.behavior == NodeBehavior::CacheOutput {
                NodeBehavior::AsFunction
            } else {
                NodeBehavior::CacheOutput
            };
        }

        if close_response.hovered() {
            close_response.show_tooltip_text("Remove node");
        }

        if close_response.clicked() {
            interaction.remove_request = Some(node.id);
            continue;
        }

        if response.clicked() || response.dragged() || body_response.clicked() {
            interaction.selection_request = Some(node.id);
        }

        let selected_id = interaction.selection_request.or(graph.selected_node_id);
        let is_selected = selected_id.is_some_and(|id| id == node.id);

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
            let button_fill = if node.behavior == NodeBehavior::CacheOutput {
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
            let button_text_color = if node.behavior == NodeBehavior::CacheOutput {
                ctx.style.cache_checked_text_color
            } else {
                visuals.text_color()
            };
            ctx.painter().text(
                cache_button_rect.center(),
                egui::Align2::CENTER_CENTER,
                button_text,
                ctx.body_font.clone(),
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

        let close_fill = if close_response.is_pointer_button_down_on() {
            visuals.widgets.active.bg_fill
        } else if close_response.hovered() {
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
    }

    interaction
}

pub fn render_ports(ctx: &RenderContext, graph: &model::GraphView) {
    for node in &graph.nodes {
        let node_width = ctx.node_width(node.id);

        for (index, _input) in node.inputs.iter().enumerate() {
            let center = node_input_pos(ctx.origin, node, index, &ctx.layout, ctx.scale);

            let port_rect = egui::Rect::from_center_size(
                center,
                egui::vec2(ctx.port_radius * 2.0, ctx.port_radius * 2.0),
            );
            let color = if ctx.ui().rect_contains_pointer(port_rect) {
                ctx.style.input_hover_color
            } else {
                ctx.style.input_port_color
            };
            ctx.painter().circle_filled(center, ctx.port_radius, color);
        }

        for (index, _output) in node.outputs.iter().enumerate() {
            let center =
                node_output_pos(ctx.origin, node, index, &ctx.layout, ctx.scale, node_width);

            let port_rect = egui::Rect::from_center_size(
                center,
                egui::vec2(ctx.port_radius * 2.0, ctx.port_radius * 2.0),
            );
            let color = if ctx.ui().rect_contains_pointer(port_rect) {
                ctx.style.output_hover_color
            } else {
                ctx.style.output_port_color
            };
            ctx.painter().circle_filled(center, ctx.port_radius, color);
        }
    }
}

pub fn render_node_labels(ctx: &RenderContext, graph: &model::GraphView) {
    let header_text_offset = ctx.style.header_text_offset;

    for node in &graph.nodes {
        let node_rect = ctx.node_rect(node);
        let node_width = ctx.node_width(node.id);

        ctx.painter().text(
            node_rect.min + egui::vec2(ctx.layout.padding, header_text_offset),
            egui::Align2::LEFT_TOP,
            &node.name,
            ctx.heading_font.clone(),
            ctx.text_color,
        );

        for (index, input) in node.inputs.iter().enumerate() {
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
                ctx.body_font.clone(),
                ctx.text_color,
            );
        }

        for (index, output) in node.outputs.iter().enumerate() {
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
                ctx.body_font.clone(),
                ctx.text_color,
            );
        }
    }
}

fn node_size(node: &model::NodeView, layout: &NodeLayout, node_width: f32) -> egui::Vec2 {
    assert!(node_width.is_finite(), "node width must be finite");
    assert!(node_width > 0.0, "node width must be positive");
    let row_count = node.inputs.len().max(node.outputs.len()).max(1);
    let height = layout.header_height
        + layout.cache_height
        + layout.padding
        + layout.row_height * row_count as f32
        + layout.padding;
    egui::vec2(node_width, height)
}

pub(crate) fn node_input_pos(
    origin: egui::Pos2,
    node: &model::NodeView,
    index: usize,
    layout: &NodeLayout,
    scale: f32,
) -> egui::Pos2 {
    assert!(
        index < node.inputs.len(),
        "input index must be within node inputs"
    );
    assert!(scale > 0.0, "graph scale must be positive");
    let y = origin.y
        + node.pos.y * scale
        + layout.header_height
        + layout.cache_height
        + layout.padding
        + layout.row_height * index as f32
        + layout.row_height * 0.5;
    egui::pos2(origin.x + node.pos.x * scale, y)
}

pub(crate) fn node_output_pos(
    origin: egui::Pos2,
    node: &model::NodeView,
    index: usize,
    layout: &NodeLayout,
    scale: f32,
    node_width: f32,
) -> egui::Pos2 {
    assert!(
        index < node.outputs.len(),
        "output index must be within node outputs"
    );
    assert!(scale > 0.0, "graph scale must be positive");
    assert!(node_width.is_finite(), "node width must be finite");
    assert!(node_width > 0.0, "node width must be positive");
    let y = origin.y
        + node.pos.y * scale
        + layout.header_height
        + layout.cache_height
        + layout.padding
        + layout.row_height * index as f32
        + layout.row_height * 0.5;
    egui::pos2(origin.x + node.pos.x * scale + node_width, y)
}

pub(crate) fn bezier_control_offset(start: egui::Pos2, end: egui::Pos2, scale: f32) -> f32 {
    assert!(scale > 0.0, "graph scale must be positive");
    let dx = (end.x - start.x).abs();
    let offset = (dx * 0.5).max(40.0 * scale);
    assert!(offset.is_finite(), "bezier control offset must be finite");
    offset
}

pub(crate) fn compute_node_widths(
    painter: &egui::Painter,
    graph: &model::GraphView,
    layout: &NodeLayout,
    heading_font: &egui::FontId,
    body_font: &egui::FontId,
    text_color: egui::Color32,
    style: &crate::gui::style::GraphStyle,
) -> HashMap<Uuid, f32> {
    layout.assert_valid();
    let scale_guess = layout.row_height / 18.0;
    assert!(scale_guess.is_finite(), "layout scale guess must be finite");
    assert!(scale_guess > 0.0, "layout scale guess must be positive");
    let mut widths = HashMap::with_capacity(graph.nodes.len());

    for node in &graph.nodes {
        let header_width =
            text_width(painter, heading_font, &node.name, text_color) + layout.padding * 2.0;
        let vertical_padding = layout.padding * style.cache_button_vertical_pad_factor;
        let cache_button_height = (layout.cache_height - vertical_padding * 2.0)
            .max(10.0 * scale_guess)
            .min(layout.cache_height);
        let cache_text_width = text_width(painter, body_font, "cached", text_color)
            .max(text_width(painter, body_font, "cache", text_color));
        let cache_button_width = (cache_button_height * style.cache_button_width_factor)
            .max(cache_button_height)
            .max(cache_text_width + layout.padding * style.cache_button_text_pad_factor * 2.0);
        let cache_row_width = if layout.cache_height > 0.0 {
            layout.padding + cache_button_width + layout.padding
        } else {
            0.0
        };
        let status_row_width = {
            let dot_diameter = style.status_dot_radius * 2.0;
            let count = 2usize;
            let gaps = (count - 1) as f32;
            let total = count as f32 * dot_diameter + gaps * style.status_item_gap;
            layout.padding + total + layout.padding
        };

        let input_widths: Vec<f32> = node
            .inputs
            .iter()
            .map(|input| text_width(painter, body_font, &input.name, text_color))
            .collect();
        let output_widths: Vec<f32> = node
            .outputs
            .iter()
            .map(|output| text_width(painter, body_font, &output.name, text_color))
            .collect();

        let row_count = node.inputs.len().max(node.outputs.len()).max(1);
        let mut max_row_width: f32 = 0.0;

        let inter_side_padding = 0.0;
        for row in 0..row_count {
            let left = input_widths.get(row).copied().unwrap_or(0.0);
            let right = output_widths.get(row).copied().unwrap_or(0.0);
            let mut row_width = layout.padding * 2.0 + left + right;
            if left > 0.0 && right > 0.0 {
                row_width += inter_side_padding;
            }
            max_row_width = max_row_width.max(row_width);
        }

        let computed = layout.node_width.max(
            header_width
                .max(max_row_width)
                .max(cache_row_width)
                .max(status_row_width),
        );
        assert!(computed.is_finite(), "node width must be finite");
        assert!(computed > 0.0, "node width must be positive");
        let prior = widths.insert(node.id, computed);
        assert!(
            prior.is_none(),
            "node width map must not contain duplicate ids"
        );
    }

    widths
}

pub(crate) fn scaled_font(ui: &egui::Ui, style: egui::TextStyle, scale: f32) -> egui::FontId {
    assert!(scale.is_finite(), "font scale must be finite");
    assert!(scale > 0.0, "font scale must be positive");
    let base = style.resolve(ui.style());
    egui::FontId {
        size: base.size * scale,
        family: base.family.clone(),
    }
}

fn text_width(
    painter: &egui::Painter,
    font: &egui::FontId,
    text: &str,
    color: egui::Color32,
) -> f32 {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    let width = galley.size().x;
    assert!(width.is_finite(), "text width must be finite");
    assert!(width >= 0.0, "text width must be non-negative");
    width
}
