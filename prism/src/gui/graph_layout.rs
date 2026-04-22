use egui::{Pos2, Vec2};
use scenarium::graph::NodeId;

use crate::gui::Gui;
use crate::gui::connection_ui::PortKind;
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

/// Per-frame layout state. The only thing cached is `NodeGalleys` —
/// shaped text that costs real work to build. Layouts (`NodeLayout`)
/// are cheap arithmetic on top of galleys + positions + style, so
/// they're computed on demand at each call site instead of stored.
#[derive(Debug, Default)]
pub struct GraphLayout {
    pub origin: Pos2,
    pub node_galleys: KeyIndexVec<NodeId, NodeGalleys>,
}

impl GraphLayout {
    /// Refreshes galley cache and the world-space `origin` for the
    /// current frame. Galleys only rebuild when their node's name or
    /// the GUI scale changes; `NodeLayout`s are not built here.
    pub fn update(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext) {
        self.origin = gui.rect.min + ctx.view_graph.pan;

        let mut compact = self.node_galleys.compact_insert_start();
        for view_node in ctx.view_graph.view_nodes.iter() {
            let node = ctx.view_graph.graph.by_id(&view_node.id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();

            let (_, galleys) = compact.insert_with(&view_node.id, || {
                NodeGalleys::new(gui, view_node.id, func, &node.name)
            });
            galleys.update(gui, func, &node.name);
        }
    }

    /// Compute the geometry for a single node from cached galleys +
    /// the caller-supplied drag offset. `update()` must have been
    /// called this frame — it populates galleys for every view-node.
    pub fn node_layout(
        &self,
        gui: &Gui<'_>,
        ctx: &GraphContext<'_>,
        node_id: &NodeId,
        drag_offset: Vec2,
    ) -> NodeLayout {
        let galleys = self.node_galleys.by_key(node_id).unwrap();
        let node = ctx.view_graph.graph.by_id(node_id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();
        let view_node = ctx.view_graph.view_nodes.by_key(node_id).unwrap();
        NodeLayout::compute(
            galleys,
            func,
            &gui.style,
            gui.scale(),
            self.origin,
            view_node.pos + drag_offset,
        )
    }

    /// Iterate layouts for every visible node with no drag offset.
    pub fn iter_layouts<'a>(
        &'a self,
        gui: &'a Gui<'_>,
        ctx: &'a GraphContext<'_>,
    ) -> impl Iterator<Item = NodeLayout> + 'a {
        ctx.view_graph
            .view_nodes
            .iter()
            .map(move |view_node| self.node_layout(gui, ctx, &view_node.id, Vec2::ZERO))
    }

    pub fn node_galleys(&self, node_id: &NodeId) -> &NodeGalleys {
        self.node_galleys.by_key(node_id).unwrap()
    }
}
