//! Topological-column auto-layout: seeds a fresh view's node positions when
//! no saved layout exists yet (a freshly loaded root, a subgraph interior on
//! first open).

use std::collections::HashMap;

use glam::Vec2;
use scenarium::graph::{Graph as CoreGraph, NodeId};

use crate::core::document::GraphView;

const AUTO_LAYOUT_COL_SPACING: f32 = 220.0;
const AUTO_LAYOUT_ROW_SPACING: f32 = 110.0;
/// Also reused by `Document::create_subgraph`'s explicit boundary-node
/// placement (its own `BOUNDARY_LAYOUT_GAP`), so this stays `pub(crate)`.
pub(crate) const AUTO_LAYOUT_ORIGIN: Vec2 = Vec2::new(40.0, 40.0);

impl GraphView {
    /// Assign positions using topological-depth columns: nodes with no
    /// bound inputs go in column 0, downstream nodes shift right by one
    /// column per max-upstream-depth. Within a column, stack vertically
    /// in graph insertion order.
    pub(crate) fn auto_layout(&mut self, graph: &CoreGraph) {
        let mut depth: HashMap<NodeId, u32> = HashMap::new();
        for node in graph.iter() {
            let d = graph
                .edges()
                .filter(|(dst, _)| dst.node_id == node.id)
                .filter_map(|(_, src)| depth.get(&src.node_id).copied().map(|d| d + 1))
                .max()
                .unwrap_or(0);
            depth.insert(node.id, d);
        }

        let mut row_in_col: HashMap<u32, u32> = HashMap::new();
        for view_node in self.view_nodes.iter_mut() {
            let d = depth.get(&view_node.id).copied().unwrap_or(0);
            let row = row_in_col.entry(d).or_insert(0);
            view_node.pos = AUTO_LAYOUT_ORIGIN
                + Vec2::new(
                    d as f32 * AUTO_LAYOUT_COL_SPACING,
                    *row as f32 * AUTO_LAYOUT_ROW_SPACING,
                );
            *row += 1;
        }
    }
}
