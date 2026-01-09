use egui::Pos2;
use graph::graph::NodeId;

use crate::gui::connection_ui::PortKind;
use crate::gui::node_layout::{self, NodeLayout};
use crate::gui::{Gui, graph_ctx::GraphContext};
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
    pub fn update(&mut self, gui: &Gui<'_>, ctx: &GraphContext) {
        let view_graph = &ctx.view_graph;
        self.origin = gui.rect.min + view_graph.pan;
        let mut compact = self.node_layouts.compact_insert_start();

        for view_node in view_graph.view_nodes.iter() {
            let (_idx, node_layout) =
                compact.insert_with(&view_node.id, || NodeLayout::new(gui, &view_node.id));

            node_layout.update(ctx, gui, self.origin);
        }
    }

    pub fn node_layout(&self, node_id: &NodeId) -> &NodeLayout {
        self.node_layouts.by_key(node_id).unwrap()
    }
}
