//! Topological-column auto-layout: seeds a fresh view's node positions when
//! no saved layout exists yet (a freshly loaded root, a subgraph interior on
//! first open).

use std::collections::HashMap;

use glam::Vec2;
use scenarium::{Graph as CoreGraph, NodeId};

use crate::core::document::{GraphView, ItemRef};

const AUTO_LAYOUT_COL_SPACING: f32 = 220.0;
const AUTO_LAYOUT_ROW_SPACING: f32 = 110.0;
/// Also reused by `Document::create_graph`'s explicit boundary-node
/// placement (its own `BOUNDARY_LAYOUT_GAP`), so this stays `pub(crate)`.
pub(crate) const AUTO_LAYOUT_ORIGIN: Vec2 = Vec2::new(40.0, 40.0);

/// Where auto-layout parks a pinned output's preview relative to its owner
/// node: above and to the right, clear of the node body (the widget is
/// 280×200 canvas units). Multiple pins on one node stagger diagonally by
/// [`PIN_LAYOUT_STAGGER`] per port index so they don't stack exactly.
const PIN_LAYOUT_OFFSET: Vec2 = Vec2::new(60.0, -240.0);
const PIN_LAYOUT_STAGGER: Vec2 = Vec2::new(28.0, 28.0);

impl GraphView {
    /// Assign positions using topological-depth columns: nodes with no
    /// bound inputs go in column 0, downstream nodes shift right by one
    /// column per max-upstream-depth. Within a column, stack vertically in
    /// the current view order. Pinned-output previews then park beside their
    /// owner node.
    pub(crate) fn auto_layout(&mut self, graph: &CoreGraph) {
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
        for item in self.item_placements.iter_mut() {
            let ItemRef::Node(id) = item.key else {
                continue;
            };
            let d = depth.get(&id).copied().unwrap_or(0);
            let row = row_in_col.entry(d).or_insert(0);
            item.pos = AUTO_LAYOUT_ORIGIN
                + Vec2::new(
                    d as f32 * AUTO_LAYOUT_COL_SPACING,
                    *row as f32 * AUTO_LAYOUT_ROW_SPACING,
                );
            *row += 1;
        }

        // Pins read their owner's just-assigned position, so this can't
        // fold into the mutable pass above; collect first, then write.
        let pin_positions: Vec<(ItemRef, Vec2)> = self
            .item_placements
            .iter()
            .filter_map(|item| {
                let ItemRef::Pin(port) = item.key else {
                    return None;
                };
                let owner = self
                    .item_placements
                    .by_key(&ItemRef::Node(port.node_id))
                    .map(|o| o.pos)
                    .unwrap_or(AUTO_LAYOUT_ORIGIN);
                let pos = owner + PIN_LAYOUT_OFFSET + PIN_LAYOUT_STAGGER * port.port_idx as f32;
                Some((item.key, pos))
            })
            .collect();
        for (key, pos) in pin_positions {
            self.item_placements.by_key_mut(&key).unwrap().pos = pos;
        }
    }
}

#[cfg(test)]
mod tests {
    use scenarium::FuncId;
    use scenarium::{Binding, InputPort, Node, NodeKind, OutputPort};

    use super::*;

    #[test]
    fn auto_layout_columns_nodes_and_parks_pins_beside_their_owner() {
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
        let (p0, p1) = (
            OutputPort::new(downstream_id, 0),
            OutputPort::new(downstream_id, 1),
        );
        graph.set_output_pinned(p0, true);
        graph.set_output_pinned(p1, true);

        let mut view = GraphView::for_graph(&graph);
        view.auto_layout(&graph);

        let pos = |key: ItemRef| view.item_placements.by_key(&key).unwrap().pos;
        let source_pos = pos(ItemRef::Node(source_id));
        let middle_pos = pos(ItemRef::Node(middle_id));
        let downstream_pos = pos(ItemRef::Node(downstream_id));
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

        let pin0 = pos(ItemRef::Pin(p0));
        let pin1 = pos(ItemRef::Pin(p1));
        assert_eq!(
            pin0,
            downstream_pos + PIN_LAYOUT_OFFSET,
            "port 0 parks at the base offset"
        );
        assert_eq!(
            pin1,
            downstream_pos + PIN_LAYOUT_OFFSET + PIN_LAYOUT_STAGGER,
            "port 1 staggers one step so the two previews don't stack"
        );
    }
}
