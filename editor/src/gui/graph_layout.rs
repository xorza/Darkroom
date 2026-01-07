use egui::Pos2;
use graph::graph::NodeId;

use crate::gui::connection_ui::PortKind;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::node_layout::{self, NodeLayout};
use crate::model::ViewGraph;
use common::key_index_vec::KeyIndexVec;

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
    pub node_layouts: KeyIndexVec<NodeId, node_layout::NodeLayout>,
}

impl Default for GraphLayout {
    fn default() -> Self {
        Self {
            origin: Pos2::ZERO,
            node_layouts: KeyIndexVec::default(),
        }
    }
}

impl GraphLayout {
    pub fn update(&mut self, ctx: &GraphContext, view_graph: &ViewGraph) {
        let rect = ctx.ui.available_rect_before_wrap();
        self.origin = rect.min + view_graph.pan;
        let mut write_idx = 0;

        for view_node in view_graph.view_nodes.iter() {
            let idx = self
                .node_layouts
                .compact_insert_with(&view_node.id, &mut write_idx, || {
                    NodeLayout::new(ctx, view_graph, &view_node.id)
                });

            self.node_layouts[idx].update(ctx, view_node, self.origin);
        }
        self.node_layouts.compact_finish(write_idx);
    }

    pub fn node_layout(&self, node_id: &NodeId) -> &NodeLayout {
        self.node_layouts.by_key(node_id).unwrap()
    }
}
