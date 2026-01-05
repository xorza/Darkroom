use egui::{Pos2, Rect};
use graph::graph::NodeId;
use hashbrown::HashMap;

use crate::gui::connection_ui::PortKind;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::node_ui;

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
        self.node_layout = node_ui::NodeLayout::from_scale(ctx.view_graph.scale);

        self.node_rects.clear();

        for view_node in ctx.view_graph.view_nodes.iter() {
            let layout =
                node_ui::compute_node_layout(ctx, &view_node.id, &self.node_layout, self.origin);
            self.node_rects.insert(view_node.id, layout.rect);
        }
    }

    pub fn node_rect(&self, node_id: &NodeId) -> Rect {
        *self
            .node_rects
            .get(node_id)
            .unwrap_or_else(|| panic!("node rect missing for {:?}", node_id))
    }

    pub fn update_node_rect_position(&mut self, view_node_id: &NodeId, new_rect: Rect) {
        self.node_rects.insert(*view_node_id, new_rect);
    }
}
