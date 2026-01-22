use egui::Pos2;
use graph::graph::NodeId;

use crate::gui::Gui;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::node_layout::NodeLayout;
use common::key_index_vec::KeyIndexVec;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRef {
    pub node_id: NodeId,
    pub port_idx: usize,
    pub kind: PortKind,
}

#[derive(Debug, Clone, Copy)]
pub struct PortInfo {
    pub port: PortRef,
    pub center: Pos2,
}

// ============================================================================
// GraphLayout
// ============================================================================

#[derive(Debug, Default)]
pub struct GraphLayout {
    pub origin: Pos2,
    pub node_layouts: KeyIndexVec<NodeId, NodeLayout>,
}

impl GraphLayout {
    pub fn update(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext) {
        self.origin = gui.rect.min + ctx.view_graph.pan;

        let mut compact = self.node_layouts.compact_insert_start();
        for view_node in ctx.view_graph.view_nodes.iter() {
            let (_idx, layout) =
                compact.insert_with(&view_node.id, || NodeLayout::new(gui, &view_node.id));
            layout.update(ctx, gui, self.origin);
        }
    }

    pub fn node_layout(&self, node_id: &NodeId) -> &NodeLayout {
        self.node_layouts.by_key(node_id).unwrap()
    }
}
