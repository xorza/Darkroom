use common::BoolExt;
use eframe::egui;
use egui::{Galley, Pos2, Rect, Vec2, pos2, vec2};
use graph::graph::NodeId;
use std::sync::Arc;

use crate::common::font::ScaledFontId;
use crate::gui::graph_ctx::GraphContext;
use crate::model::ViewGraph;
use common::key_index_vec::KeyIndexKey;

#[derive(Debug, Clone)]
pub struct NodeLayout {
    inited: bool,
    scale: f32,

    pub node_id: NodeId,
    pub body_rect: Rect,
    pub remove_btn_rect: Rect,
    pub cache_button_rect: Rect,
    pub dot_first_center: Pos2,
    pub input_first_center: Pos2,
    pub output_first_center: Pos2,
    pub port_row_height: f32,
    pub port_activation_radius: f32,
    pub header_row_height: f32,
    pub title_galley: Arc<Galley>,
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

    pub fn new(ctx: &GraphContext, node_id: &NodeId) -> NodeLayout {
        let title_galley = ctx.painter.layout_no_wrap(
            String::default(),
            ctx.style.body_font.clone(),
            ctx.style.text_color,
        );

        NodeLayout {
            inited: false,
            scale: 1.0,
            node_id: *node_id,
            body_rect: Rect::ZERO,
            remove_btn_rect: Rect::ZERO,
            cache_button_rect: Rect::ZERO,
            dot_first_center: Pos2::ZERO,
            input_first_center: Pos2::ZERO,
            output_first_center: Pos2::ZERO,
            port_row_height: 0.0,
            port_activation_radius: 0.0,
            header_row_height: 0.0,
            title_galley,
            input_galleys: Vec::default(),
            output_galleys: Vec::default(),
        }
    }

    pub fn update(&mut self, ctx: &GraphContext, view_graph: &ViewGraph, origin: Pos2) {
        let view_node = view_graph.view_nodes.by_key(&self.node_id).unwrap();
        let node = view_graph.graph.by_id(&self.node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();

        if !self.inited || crate::common::scale_changed(self.scale, ctx.scale) {
            self.scale = ctx.scale;
            self.inited = true;

            self.title_galley = ctx.painter.layout_no_wrap(
                node.name.to_string(),
                ctx.style.body_font.scaled(self.scale),
                ctx.style.text_color,
            );

            let label_font = ctx.style.sub_font.scaled(self.scale);

            self.input_galleys.clear();
            for input in &func.inputs {
                let galley = ctx.painter.layout_no_wrap(
                    input.name.to_string(),
                    label_font.clone(),
                    ctx.style.text_color,
                );
                self.input_galleys.push(galley);
            }
            self.output_galleys.clear();
            for output in &func.outputs {
                let galley = ctx.painter.layout_no_wrap(
                    output.name.to_string(),
                    label_font.clone(),
                    ctx.style.text_color,
                );
                self.output_galleys.push(galley);
            }
        }

        // ===============
        assert!(self.inited);

        let padding = ctx.style.padding * self.scale;
        let small_padding = ctx.style.small_padding * self.scale;

        let title_width = self.title_galley.size().x + padding * 2.0;
        let remove_size = ctx.style.node.remove_btn_size * self.scale + small_padding * 2.0;
        let status_dot_size = ctx.style.node.status_dot_radius * self.scale * 2.0;
        let header_height = self
            .title_galley
            .size()
            .y
            .max(remove_size)
            .max(status_dot_size);

        let header_width = {
            let status_width = 2.0 * (small_padding + status_dot_size);

            title_width + padding + status_width + padding + remove_size + padding
        };

        let input_count = self.input_galleys.len();
        let output_count = self.output_galleys.len();
        let row_count = input_count.max(output_count).max(1);
        let port_label_side_padding = ctx.style.node.port_label_side_padding * self.scale;
        let mut max_row_width: f32 = 0.0;
        for row in 0..row_count {
            let left = self
                .input_galleys
                .get(row)
                .map_or(0.0, |galley| galley.size().x + port_label_side_padding);
            let right = self
                .output_galleys
                .get(row)
                .map_or(0.0, |galley| galley.size().x + port_label_side_padding);

            let row_width = left + right + (left > 0.0 && right > 0.0).then_else(padding, 0.0);
            max_row_width = max_row_width.max(row_width);
        }

        let cache_button_height = ctx.style.sub_font.size * self.scale;

        let header_row_height = header_height + padding * 2.0;
        let port_row_height = ctx
            .style
            .sub_font
            .size
            .max(ctx.style.node.port_radius * 2.0)
            * self.scale
            + small_padding;
        let cache_row_height = cache_button_height + small_padding * 2.0;

        let node_width = header_width.max(max_row_width);
        let node_height = header_row_height
            + cache_row_height
            + port_row_height * row_count as f32
            + padding * 2.0;
        let node_size = vec2(node_width, node_height);
        let body_rect = Rect::from_min_size(Pos2::ZERO, node_size);

        let remove_pos = egui::pos2(
            body_rect.max.x - padding - remove_size,
            body_rect.min.y + padding,
        );
        let remove_rect = Rect::from_min_size(remove_pos, Vec2::ONE * remove_size);

        let dot_radius = ctx.style.node.status_dot_radius * self.scale;
        let dot_first_center = {
            let dot_x = remove_rect.min.x - padding - dot_radius;
            let dot_center_y = header_row_height * 0.5;
            egui::pos2(dot_x, dot_center_y)
        };

        let cache_button_rect = Rect::from_min_size(
            pos2(
                body_rect.min.x + padding,
                body_rect.min.y + header_row_height + small_padding,
            ),
            vec2(
                ctx.style.node.cache_btn_width * self.scale,
                cache_button_height,
            ),
        );

        let base_y = body_rect.min.y
            + header_row_height
            + cache_row_height
            + padding
            + port_row_height * 0.5;
        let input_first_center = egui::pos2(body_rect.min.x, base_y);
        let output_first_center = egui::pos2(body_rect.min.x + node_width, base_y);

        let global_offset = (origin + view_node.pos.to_vec2() * self.scale).to_vec2();

        let body_rect = body_rect.translate(global_offset);
        let remove_btn_rect = remove_rect.translate(global_offset);
        let cache_button_rect = cache_button_rect.translate(global_offset);
        let dot_first_center = dot_first_center + global_offset;
        let input_first_center = input_first_center + global_offset;
        let output_first_center = output_first_center + global_offset;

        self.body_rect = body_rect;
        self.remove_btn_rect = remove_btn_rect;
        self.cache_button_rect = cache_button_rect;
        self.dot_first_center = dot_first_center;
        self.input_first_center = input_first_center;
        self.output_first_center = output_first_center;
        self.port_row_height = port_row_height;
        self.port_activation_radius = port_row_height * 0.5;
        self.header_row_height = header_row_height;
    }
}

impl KeyIndexKey<NodeId> for NodeLayout {
    fn key(&self) -> &NodeId {
        &self.node_id
    }
}
