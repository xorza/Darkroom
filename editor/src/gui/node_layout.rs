use eframe::egui;
use egui::{Color32, FontId, Galley, Pos2, Rect, vec2};
use graph::graph::NodeId;
use std::sync::Arc;

use crate::common::font::ScaledFontId;
use crate::gui::graph_ctx::GraphContext;
use crate::model::ViewGraph;
use common::key_index_vec::KeyIndexKey;

#[derive(Debug, Clone)]
pub struct NodeLayout {
    pub node_id: NodeId,
    pub body_rect: Rect,
    pub remove_btn_rect: Rect,
    pub cache_button_rect: Rect,
    pub dot_first_center: Pos2,
    pub input_first_center: Pos2,
    pub output_first_center: Pos2,
    pub port_row_height: f32,
    pub padding: f32,
    pub title_galley: Arc<Galley>,
    pub cache_btn_galley: Arc<Galley>,
    pub input_galleys: Vec<Arc<Galley>>,
    pub output_galleys: Vec<Arc<Galley>>,
}

impl NodeLayout {
    pub fn input_center(&self, index: usize) -> Pos2 {
        egui::pos2(
            self.input_first_center.x,
            self.input_first_center.y + self.port_row_height * index as f32,
        )
    }

    pub fn output_center(&self, index: usize) -> Pos2 {
        egui::pos2(
            self.output_first_center.x,
            self.output_first_center.y + self.port_row_height * index as f32,
        )
    }

    pub fn dot_center(&self, index: usize, dot_step: f32) -> Pos2 {
        let first = self.dot_first_center;
        egui::pos2(first.x - dot_step * index as f32, first.y)
    }

    pub fn new(ctx: &GraphContext, view_graph: &ViewGraph, view_node_id: &NodeId) -> NodeLayout {
        let node = view_graph.graph.by_id(view_node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();
        let scale = ctx.scale;

        let title_galley = ctx.painter.layout_no_wrap(
            node.name.to_string(),
            ctx.style.heading_font.scaled(scale),
            ctx.style.text_color,
        );

        let mut input_galleys = Vec::with_capacity(func.inputs.len());
        let mut output_galleys = Vec::with_capacity(func.outputs.len());

        let label_font = ctx.style.sub_font.scaled(scale);
        let cache_btn_galley = ctx.painter.layout_no_wrap(
            "cache".to_string(),
            ctx.style.body_font.scaled(scale),
            ctx.style.text_color,
        );
        for input in &func.inputs {
            let galley = ctx.painter.layout_no_wrap(
                input.name.to_string(),
                label_font.clone(),
                ctx.style.text_color,
            );
            input_galleys.push(galley);
        }
        for output in &func.outputs {
            let galley = ctx.painter.layout_no_wrap(
                output.name.to_string(),
                label_font.clone(),
                ctx.style.text_color,
            );
            output_galleys.push(galley);
        }

        NodeLayout {
            node_id: *view_node_id,
            body_rect: Rect::ZERO,
            remove_btn_rect: Rect::ZERO,
            cache_button_rect: Rect::ZERO,
            dot_first_center: Pos2::ZERO,
            input_first_center: Pos2::ZERO,
            output_first_center: Pos2::ZERO,
            port_row_height: 0.0,
            padding: 0.0,
            title_galley,
            cache_btn_galley,
            input_galleys,
            output_galleys,
        }
    }

    pub fn update(
        &mut self,
        ctx: &GraphContext,
        view_graph: &ViewGraph,
        view_node_id: &NodeId,
        origin: Pos2,
    ) {
        let view_node = view_graph.view_nodes.by_key(view_node_id).unwrap();

        let scale = ctx.scale;
        let padding = ctx.style.node.padding * scale;

        let header_height = ctx.style.heading_font.size * scale;
        let remove_btn_size = header_height;

        let title_width = self.title_galley.size().x + padding * 2.0;

        let header_width = {
            let status_width = 2.0 * (padding + ctx.style.node.status_dot_radius * 2.0);
            let remove_width = remove_btn_size + padding * 2.0;

            title_width + status_width + remove_width
        };

        let input_count = self.input_galleys.len();
        let output_count = self.output_galleys.len();
        let row_count = input_count.max(output_count).max(1);
        let mut max_row_width: f32 = 0.0;
        for row in 0..row_count {
            let left = self
                .input_galleys
                .get(row)
                .map_or(0.0, |galley| galley.size().x + padding);
            let right = self
                .output_galleys
                .get(row)
                .map_or(0.0, |galley| galley.size().x + padding);
            let mut row_width = padding * 2.0 + left + right;
            if left > 0.0 && right > 0.0 {
                row_width += padding;
            }
            max_row_width = max_row_width.max(row_width);
        }

        let cache_btn_width = self.cache_btn_galley.size().x + padding * 2.0;
        let cache_button_height = ctx.style.body_font.size * scale + padding * 2.0;

        let header_row_height = header_height + padding * 2.0;
        let port_row_height = ctx.style.sub_font.size * scale + padding * 2.0;
        let cache_row_height = cache_button_height + padding * 2.0;
        let cache_row_width = cache_btn_width + padding * 2.0;

        let node_width = header_width.max(max_row_width).max(cache_row_width);
        let node_height = header_row_height
            + cache_row_height
            + port_row_height * row_count as f32
            + padding * 2.0;
        let node_size = vec2(node_width, node_height);
        let body_rect = Rect::from_min_size(Pos2::ZERO, node_size);

        let header_rect = Rect::from_min_size(body_rect.min, vec2(title_width, header_row_height));

        let remove_pos = egui::pos2(
            body_rect.max.x - padding - remove_btn_size,
            body_rect.min.y + padding,
        );
        let remove_rect = Rect::from_min_size(remove_pos, vec2(remove_btn_size, remove_btn_size));

        let dot_radius = scale * ctx.style.node.status_dot_radius;
        let dot_first_center = {
            let dot_x = remove_rect.min.x - padding - dot_radius;
            let dot_center_y = header_rect.center().y;
            egui::pos2(dot_x, dot_center_y)
        };

        let cache_button_rect = Rect::from_min_size(
            egui::pos2(
                body_rect.min.x + padding,
                body_rect.min.y
                    + header_row_height
                    + (cache_row_height - cache_button_height) * 0.5,
            ),
            vec2(cache_btn_width, cache_button_height),
        );

        let base_y = body_rect.min.y
            + header_row_height
            + cache_row_height
            + padding
            + port_row_height * 0.5;
        let input_first_center = egui::pos2(body_rect.min.x, base_y);
        let output_first_center = egui::pos2(body_rect.min.x + node_width, base_y);

        let global_offset = (origin + view_node.pos.to_vec2() * scale).to_vec2();

        let body_rect = body_rect.translate(global_offset);
        let remove_btn_rect = remove_rect.translate(global_offset);
        let cache_button_rect = cache_button_rect.translate(global_offset);
        let dot_first_center = dot_first_center + global_offset;
        let input_first_center = input_first_center + global_offset;
        let output_first_center = output_first_center + global_offset;

        self.node_id = *view_node_id;
        self.body_rect = body_rect;
        self.remove_btn_rect = remove_btn_rect;
        self.cache_button_rect = cache_button_rect;
        self.dot_first_center = dot_first_center;
        self.input_first_center = input_first_center;
        self.output_first_center = output_first_center;
        self.port_row_height = port_row_height;
        self.padding = padding;
    }
}

impl KeyIndexKey<NodeId> for NodeLayout {
    fn key(&self) -> &NodeId {
        &self.node_id
    }
}

pub(crate) fn text_width(
    painter: &egui::Painter,
    font: &FontId,
    text: &str,
    color: Color32,
) -> f32 {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    galley.size().x
}
