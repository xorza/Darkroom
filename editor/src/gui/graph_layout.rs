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
    pub node_layouts: HashMap<NodeId, node_ui::NodeLayout>,
}

impl Default for GraphLayout {
    fn default() -> Self {
        Self {
            origin: Pos2::ZERO,
            node_layouts: HashMap::new(),
        }
    }
}

impl GraphLayout {
    pub fn update(&mut self, ctx: &GraphContext) {
        let rect = ctx.ui.available_rect_before_wrap();
        self.origin = rect.min + ctx.view_graph.pan;
        let base_layout = node_ui::NodeLayout::from_scale(ctx.view_graph.scale);
        node_ui::compute_node_layouts(ctx, &base_layout, self.origin, &mut self.node_layouts);
    }

    pub fn node_rect(&self, node_id: &NodeId) -> Rect {
        self.node_layout(node_id).rect
    }

    pub fn node_layout(&self, node_id: &NodeId) -> &node_ui::NodeLayout {
        self.node_layouts
            .get(node_id)
            .unwrap_or_else(|| panic!("node layout missing for {:?}", node_id))
    }

    pub fn update_node_layout(&mut self, view_node_id: &NodeId, new_layout: node_ui::NodeLayout) {
        self.node_layouts.insert(*view_node_id, new_layout);
    }
}
