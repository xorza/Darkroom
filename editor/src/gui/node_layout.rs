use common::BoolExt;
use common::key_index_vec::KeyIndexKey;
use eframe::egui;
use egui::{FontId, Galley, Pos2, Rect, Vec2, pos2, vec2};
use graph::graph::NodeId;
use graph::prelude::FuncBehavior;
use std::sync::Arc;

use crate::common::UiEquals;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::PortRef;
use crate::gui::{Gui, graph_ctx::GraphContext};

#[derive(Debug)]
pub struct NodeLayout {
    inited: bool,
    scale: f32,

    pub node_id: NodeId,
    pub body_rect: Rect,
    pub remove_btn_rect: Rect,

    pub has_cache_btn: bool,
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
    pub event_galleys: Vec<Arc<Galley>>,
}

impl NodeLayout {
    pub fn new(gui: &Gui<'_>, node_id: &NodeId) -> NodeLayout {
        let title_galley = gui.painter().layout_no_wrap(
            String::default(),
            gui.style.body_font.clone(),
            gui.style.text_color,
        );

        NodeLayout {
            inited: false,
            scale: 1.0,
            node_id: *node_id,
            body_rect: Rect::ZERO,
            remove_btn_rect: Rect::ZERO,
            has_cache_btn: false,
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
            event_galleys: Vec::default(),
        }
    }

    // === Port position accessors ===

    pub fn input_center(&self, index: usize) -> Pos2 {
        self.port_at_row(self.input_first_center, index)
    }

    pub fn output_center(&self, index: usize) -> Pos2 {
        self.port_at_row(self.output_first_center, index)
    }

    pub fn event_center(&self, index: usize) -> Pos2 {
        self.port_at_row(self.output_first_center, index + self.output_galleys.len())
    }

    pub fn trigger_center(&self) -> Pos2 {
        self.body_rect.min
    }

    pub fn dot_center(&self, index: usize, dot_step: f32) -> Pos2 {
        pos2(
            self.dot_first_center.x - dot_step * index as f32,
            self.dot_first_center.y,
        )
    }

    pub fn port_center(&self, port: &PortRef) -> Pos2 {
        match port.kind {
            PortKind::Input => self.input_center(port.port_idx),
            PortKind::Output => self.output_center(port.port_idx),
            PortKind::Event => self.event_center(port.port_idx),
            PortKind::Trigger => self.trigger_center(),
        }
    }

    fn port_at_row(&self, base: Pos2, row: usize) -> Pos2 {
        pos2(base.x, base.y + self.port_row_height * row as f32)
    }

    // === Update logic ===

    pub fn update(&mut self, ctx: &GraphContext, gui: &mut Gui, origin: Pos2) {
        let view_node = ctx.view_graph.view_nodes.by_key(&self.node_id).unwrap();
        let node = ctx.view_graph.graph.by_id(&self.node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();

        let scale_changed = !self.scale.ui_equals(gui.scale());

        if self.title_galley.text() != node.name || scale_changed {
            self.title_galley = gui.painter().layout_no_wrap(
                node.name.clone(),
                gui.style.body_font.clone(),
                gui.style.text_color,
            );
        }

        if !self.inited || scale_changed {
            self.scale = gui.scale();
            self.inited = true;
            self.rebuild_port_galleys(gui, func);
        }

        assert!(self.inited);
        self.compute_layout(gui, func, origin, view_node.pos);
    }

    fn rebuild_port_galleys(&mut self, gui: &Gui, func: &graph::prelude::Func) {
        let font = &gui.style.sub_font;

        self.input_galleys = func
            .inputs
            .iter()
            .map(|p| self.make_galley(gui, &p.name, font))
            .collect();

        self.output_galleys = func
            .outputs
            .iter()
            .map(|p| self.make_galley(gui, &p.name, font))
            .collect();

        self.event_galleys = func
            .events
            .iter()
            .map(|e| self.make_galley(gui, &e.name, font))
            .collect();
    }

    fn make_galley(&self, gui: &Gui, text: &str, font: &FontId) -> Arc<Galley> {
        gui.painter()
            .layout_no_wrap(text.to_string(), font.clone(), gui.style.text_color)
    }

    fn compute_layout(
        &mut self,
        gui: &mut Gui,
        func: &graph::prelude::Func,
        origin: Pos2,
        node_pos: egui::Pos2,
    ) {
        // Extract style values upfront to avoid borrow conflicts
        let padding = gui.style.padding;
        let small_padding = gui.style.small_padding;
        let remove_btn_size = gui.style.node.remove_btn_size;
        let status_dot_radius = gui.style.node.status_dot_radius;
        let port_label_side_padding = gui.style.node.port_label_side_padding;
        let port_radius = gui.style.node.port_radius;
        let cache_btn_width = gui.style.node.cache_btn_width;
        let sub_font = gui.style.sub_font.clone();
        let sub_font_size = sub_font.size;
        let row_height = gui.font_height(&sub_font) + small_padding;

        // Header dimensions
        let remove_size = remove_btn_size + small_padding * 2.0;
        let status_dot_size = status_dot_radius * 2.0;
        let header_height = self
            .title_galley
            .size()
            .y
            .max(remove_size)
            .max(status_dot_size);

        let title_width = self.title_galley.size().x + padding * 2.0;
        let status_width = 2.0 * (small_padding + status_dot_size);
        let header_width = title_width + padding + status_width + padding + remove_size + padding;

        // Port row dimensions
        let row_count = self
            .input_galleys
            .len()
            .max(self.output_galleys.len() + self.event_galleys.len())
            .max(1);

        let (max_left, max_right) = self.compute_max_galley_widths(row_count);
        let row_width = port_label_side_padding * 2.0
            + max_left
            + max_right
            + (max_left > 0.0 && max_right > 0.0).then_else(padding, 0.0);

        // Cache button
        let has_cache_btn = !(func.terminal
            || func.outputs.is_empty()
            || (func.behavior == FuncBehavior::Pure && func.inputs.is_empty()));
        let cache_row_height = if has_cache_btn {
            sub_font_size + padding * 2.0
        } else {
            0.0
        };

        // Final dimensions
        let header_row_height = header_height + small_padding * 2.0;
        let port_row_height = row_height.max(port_radius * 2.0);

        let node_width = header_width.max(row_width).max(80.0 * self.scale);
        let node_height = header_row_height
            + cache_row_height
            + port_row_height * row_count as f32
            + padding * 2.0;

        // Build local rects
        let body_rect = Rect::from_min_size(Pos2::ZERO, vec2(node_width, node_height));

        let remove_rect = Rect::from_min_size(
            pos2(
                body_rect.max.x - padding - remove_size,
                body_rect.min.y + padding,
            ),
            Vec2::splat(remove_size),
        );

        let dot_first_center = pos2(
            remove_rect.min.x - padding - status_dot_radius,
            header_row_height * 0.5,
        );

        let cache_button_rect = Rect::from_min_size(
            pos2(padding, header_row_height + padding),
            vec2(cache_btn_width, sub_font_size),
        );

        let port_base_y = header_row_height + cache_row_height + padding + port_row_height * 0.5;
        let input_first_center = pos2(0.0, port_base_y);
        let output_first_center = pos2(node_width, port_base_y);

        // Apply global offset
        let global_offset = (origin + node_pos.to_vec2() * self.scale).to_vec2();

        self.body_rect = body_rect.translate(global_offset);
        self.remove_btn_rect = remove_rect.translate(global_offset);
        self.has_cache_btn = has_cache_btn;
        self.cache_button_rect = cache_button_rect.translate(global_offset);
        self.dot_first_center = dot_first_center + global_offset;
        self.input_first_center = input_first_center + global_offset;
        self.output_first_center = output_first_center + global_offset;
        self.port_row_height = port_row_height;
        self.port_activation_radius = port_row_height * 0.5;
        self.header_row_height = header_row_height;
    }

    fn compute_max_galley_widths(&self, row_count: usize) -> (f32, f32) {
        let mut max_left: f32 = 0.0;
        let mut max_right: f32 = 0.0;

        for row in 0..row_count {
            let left = self.input_galleys.get(row).map_or(0.0, |g| g.size().x);

            let right = if row < self.output_galleys.len() {
                self.output_galleys.get(row).map_or(0.0, |g| g.size().x)
            } else {
                let event_row = row - self.output_galleys.len();
                self.event_galleys
                    .get(event_row)
                    .map_or(0.0, |g| g.size().x)
            };

            max_left = max_left.max(left);
            max_right = max_right.max(right);
        }

        (max_left, max_right)
    }
}

impl KeyIndexKey<NodeId> for NodeLayout {
    fn key(&self) -> &NodeId {
        &self.node_id
    }
}
