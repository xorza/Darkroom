use egui::Pos2;
use scenarium::graph::NodeId;

use crate::gui::Gui;
use crate::gui::connection_ui::PortKind;
use crate::gui::gesture::Gesture;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::node_layout::{NodeGalleys, NodeLayout};
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

/// Per-frame layout state for every node in the view.
///
/// `node_galleys` holds the only thing expensive to build — shaped text —
/// and is updated lazily when a node's name or the GUI scale changes.
/// `node_layouts` is purely derived from galleys + position + style and
/// is recomputed from scratch every frame; it is stored only so that the
/// next frame's interaction pass can hit-test against the previous
/// frame's rects.
#[derive(Debug, Default)]
pub struct GraphLayout {
    pub origin: Pos2,
    pub node_galleys: KeyIndexVec<NodeId, NodeGalleys>,
    pub node_layouts: KeyIndexVec<NodeId, NodeLayout>,
}

impl GraphLayout {
    pub fn update(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext, gesture: &Gesture) {
        self.origin = gui.rect.min + ctx.view_graph.pan;

        let mut compact_galleys = self.node_galleys.compact_insert_start();
        let mut compact_layouts = self.node_layouts.compact_insert_start();

        for view_node in ctx.view_graph.view_nodes.iter() {
            let node = ctx.view_graph.graph.by_id(&view_node.id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();

            let (_, galleys) = compact_galleys.insert_with(&view_node.id, || {
                NodeGalleys::new(gui, view_node.id, func, &node.name)
            });
            galleys.update(gui, func, &node.name);

            let drag_offset = gesture.node_drag_offset_for(&view_node.id);
            let layout = NodeLayout::compute(
                view_node.id,
                galleys,
                func,
                gui,
                self.origin,
                view_node.pos + drag_offset,
            );
            let (_, slot) = compact_layouts.insert_with(&view_node.id, || layout);
            *slot = layout;
        }
    }

    pub fn node_layout(&self, node_id: &NodeId) -> &NodeLayout {
        self.node_layouts.by_key(node_id).unwrap()
    }

    pub fn node_galleys(&self, node_id: &NodeId) -> &NodeGalleys {
        self.node_galleys.by_key(node_id).unwrap()
    }
}
