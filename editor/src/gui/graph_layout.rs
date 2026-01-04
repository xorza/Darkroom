use egui::{Pos2, Rect};
use graph::graph::NodeId;
use hashbrown::HashMap;

use crate::gui::connection_ui::PortKind;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::node_ui;
use crate::model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRef {
    pub node_id: NodeId,
    pub idx: usize,
    pub kind: PortKind,
}

#[derive(Debug, Clone, Copy)]
pub struct PortInfo {
    pub port: PortRef,
    pub center: Pos2,
}

#[derive(Debug)]
pub struct GraphLayout {
    pub origin: Pos2,
    pub node_layout: node_ui::NodeLayout,
    pub node_rects: HashMap<NodeId, Rect>,
}

impl Default for GraphLayout {
    fn default() -> Self {
        Self {
            origin: Pos2::ZERO,
            node_layout: node_ui::NodeLayout::default(),
            node_rects: HashMap::new(),
        }
    }
}

impl GraphLayout {
    pub fn update(&mut self, ctx: &GraphContext) {
        let rect = ctx.ui.available_rect_before_wrap();
        self.origin = rect.min + ctx.view_graph.pan;
        self.node_layout = node_ui::NodeLayout::default().scaled(ctx.view_graph.scale);

        node_ui::compute_node_rects(ctx, &self.node_layout, self.origin, &mut self.node_rects);
    }

    pub fn node_width(&self, node_id: &NodeId) -> f32 {
        self.node_rects
            .get(node_id)
            .copied()
            .expect("node width must be precomputed for view node")
            .width()
    }

    pub fn node_rect(&self, node_id: &NodeId) -> Rect {
        *self.node_rects.get(node_id).unwrap()
    }

    pub fn update_node_rect_position(&mut self, view_node: &model::ViewNode, scale: f32) {
        let rect = self.node_rect(&view_node.id);
        let size = rect.size();
        let min = self.origin + view_node.pos.to_vec2() * scale;
        self.node_rects
            .insert(view_node.id, Rect::from_min_size(min, size));
    }
}
