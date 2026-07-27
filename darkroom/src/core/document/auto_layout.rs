//! Topological-column auto-layout: seeds a fresh view's node positions when
//! no saved layout exists yet (a freshly loaded root, a graph interior on
//! first open).

use std::collections::HashMap;

use glam::Vec2;
use scenarium::{Graph as CoreGraph, NodeId};

use crate::core::document::GraphView;

const AUTO_LAYOUT_COL_SPACING: f32 = 220.0;
const AUTO_LAYOUT_ROW_SPACING: f32 = 110.0;
/// Also reused by `Document::create_graph`'s explicit boundary-node
/// placement (its own `BOUNDARY_LAYOUT_GAP`).
pub(super) const AUTO_LAYOUT_ORIGIN: Vec2 = Vec2::new(40.0, 40.0);

impl GraphView {
    /// Assign positions using topological-depth columns: nodes with no
    /// bound inputs go in column 0, downstream nodes shift right by one
    /// column per max-upstream-depth. Within a column, stack vertically in
    /// the current view order.
    pub(super) fn auto_layout(&mut self, graph: &CoreGraph) {
        let mut depth: HashMap<NodeId, u32> = graph.iter().map(|node| (node.id, 0)).collect();
        for _ in 0..graph.len().saturating_sub(1) {
            let mut changed = false;
            for (dst, src) in graph.edges() {
                let candidate = depth.get(&src.node_id).copied().unwrap() + 1;
                let current = depth.get_mut(&dst.node_id).unwrap();
                if candidate > *current {
                    *current = candidate;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut row_in_col: HashMap<u32, u32> = HashMap::new();
        for (key, position) in &mut self.item_placements {
            let id = *key;
            let d = depth.get(&id).copied().unwrap_or(0);
            let row = row_in_col.entry(d).or_insert(0);
            *position = AUTO_LAYOUT_ORIGIN
                + Vec2::new(
                    d as f32 * AUTO_LAYOUT_COL_SPACING,
                    *row as f32 * AUTO_LAYOUT_ROW_SPACING,
                );
            *row += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use scenarium::FuncId;
    use scenarium::{Binding, InputPort, Node, NodeKind};

    use super::*;

    #[test]
    fn auto_layout_columns_nodes_by_topological_depth() {
        let mut graph = CoreGraph::default();
        for _ in 0..3 {
            graph.add(Node::new(NodeKind::Func(FuncId::unique())));
        }
        let iteration_ids: [NodeId; 3] = graph
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let [downstream_id, middle_id, source_id] = iteration_ids;
        graph.set_input_binding(InputPort::new(middle_id, 0), Binding::bind(source_id, 0));
        graph.set_input_binding(
            InputPort::new(downstream_id, 0),
            Binding::bind(middle_id, 0),
        );
        let mut view = GraphView::for_graph(&graph);
        view.auto_layout(&graph);

        let pos = |key: NodeId| *view.item_placements.get(&key).unwrap();
        let source_pos = pos(source_id);
        let middle_pos = pos(middle_id);
        let downstream_pos = pos(downstream_id);
        assert_eq!(
            source_pos, AUTO_LAYOUT_ORIGIN,
            "source node in column 0, row 0"
        );
        assert_eq!(
            middle_pos,
            AUTO_LAYOUT_ORIGIN + Vec2::new(AUTO_LAYOUT_COL_SPACING, 0.0),
            "middle node one column right"
        );
        assert_eq!(
            downstream_pos,
            AUTO_LAYOUT_ORIGIN + Vec2::new(AUTO_LAYOUT_COL_SPACING * 2.0, 0.0),
            "downstream node two columns right"
        );
    }
}
