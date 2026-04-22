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

/// Per-frame layout state. Caches only `NodeGalleys` (shaped text).
/// `NodeLayout` is cheap arithmetic on top of galleys + positions +
/// style and is computed on demand at each call site.
#[derive(Debug, Default)]
pub struct GraphLayout {
    node_galleys: KeyIndexVec<NodeId, NodeGalleys>,
}

/// World-space origin (where graph coord (0, 0) lands on screen) for
/// the current frame. Derived; kept out of `GraphLayout` so there's
/// no cached-then-stale foot-gun.
pub fn origin(gui: &Gui<'_>, ctx: &GraphContext<'_>) -> Pos2 {
    gui.rect.min + ctx.view_graph.pan
}

impl GraphLayout {
    /// Refresh galley cache: rebuild any stale entry (name or GUI
    /// scale changed) and insert entries for newly added nodes.
    /// Must be called once per frame before any `node_layout` read.
    pub fn refresh_galleys(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext) {
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

    /// Compute geometry for a single node from cached galleys + the
    /// caller-supplied drag offset. `refresh_galleys()` must have run
    /// this frame — it populates galleys for every view-node.
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
            origin(gui, ctx),
            view_node.pos + drag_offset,
        )
    }

    pub fn node_galleys(&self, node_id: &NodeId) -> &NodeGalleys {
        self.node_galleys.by_key(node_id).unwrap()
    }
}
