//! [`GraphView`]: how the editor places, stacks and frames the graph.

use std::collections::{BTreeMap, BTreeSet};

use glam::Vec2;
use scenarium::{Graph as CoreGraph, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::error::GraphViewValidationError;
use crate::core::document::viewport::Viewport;

/// Where one node body sits and how far forward it paints.
///
/// The two are independent: dragging a node changes `pos` and nothing else,
/// raising it changes `z` and nothing else. Keeping the stacking order as a
/// *value* here rather than as the position of an entry in an ordered map is
/// what lets [`GraphView`] derive its equality and serde, and what makes
/// `Raise` a field write instead of a reorder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ItemPlacement {
    /// Canvas-world position of the body's top-left corner.
    pub(crate) pos: Vec2,
    /// Paint depth: higher paints later, so it sits in front. Ties break by
    /// `NodeId`, so the order is total and stable across frames — see
    /// [`GraphView::paint_order`].
    pub(crate) z: u32,
}

/// One entry of a [`GraphView::paint_order`] sweep, sorted by its own derived
/// order — depth first, then id.
///
/// The tie-break on `id` is what makes the order *total*: several items share
/// the seed `z` of 0 until something raises them, and a comparator that let
/// those tie would leave their relative order at the mercy of the sort's
/// stability. Carrying `z` alongside the id is what keeps the sort from
/// descending [`GraphView::item_placements`] once per comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StackedItem {
    /// Paint depth: higher paints later, so it sits in front.
    pub(crate) z: u32,
    pub(crate) id: NodeId,
}

/// Editor-side view metadata for the document's graph: per-item placements,
/// the viewport, and the selection (`Document::main_view`). The graph *data*
/// itself lives in the core `Graph`; this is purely how the editor presents
/// and navigates it.
///
/// **Everything here is persisted and undoable, by design** — reopening
/// a file restores the exact camera and selection, and Ctrl+Z walks
/// camera/selection changes alongside structural edits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GraphView {
    /// Every node body's placement. Exactly one entry per graph node —
    /// enforced by [`Self::validate`].
    ///
    /// Order-insensitive on purpose: paint order rides in each entry's
    /// [`ItemPlacement::z`], so this map is free to be keyed and compared as a
    /// set. Ten of the eleven callers that walk the nodes don't care about
    /// stacking at all, and the one that does asks for [`Self::paint_order`].
    pub(crate) item_placements: BTreeMap<NodeId, ItemPlacement>,
    pub(crate) viewport: Viewport,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering).
    pub(crate) selected: BTreeSet<NodeId>,
}

impl GraphView {
    /// A fresh view over `graph`: one zero-positioned, unraised placement per
    /// node, the default camera, nothing selected.
    ///
    /// The only way a view is built from a graph, so there is nowhere for the
    /// two to disagree about which nodes are placed — which is exactly what
    /// [`Self::validate`] checks on the way back in from disk.
    pub(crate) fn new(graph: &CoreGraph) -> Self {
        Self {
            item_placements: graph
                .iter()
                .map(|node| (node.id, ItemPlacement::default()))
                .collect(),
            ..Self::default()
        }
    }

    /// The items back-to-front into `out`: what a paint pass draws in order,
    /// and the only place stacking is resolved. `out` is cleared first, so a
    /// caller hands the same buffer over every frame.
    ///
    /// Fills a caller-owned buffer rather than returning one because the
    /// production caller is the per-frame node draw: a fresh `Vec` here would
    /// be one allocation per graph pane per frame, sized to the whole graph
    /// and taken before culling drops the off-screen nodes.
    pub(crate) fn paint_order(&self, out: &mut Vec<StackedItem>) {
        out.clear();
        out.reserve_exact(self.item_placements.len());
        out.extend(
            self.item_placements
                .iter()
                .map(|(id, placement)| StackedItem {
                    z: placement.z,
                    id: *id,
                }),
        );
        out.sort_unstable();
    }

    /// The depth that puts an item in front of every other. One past the
    /// current maximum, so a raise never has to renumber its neighbours.
    pub(crate) fn front_z(&self) -> u32 {
        self.item_placements
            .values()
            .map(|placement| placement.z)
            .max()
            .map_or(0, |top| top.saturating_add(1))
    }

    /// The depth that puts `key` in front of every *other* item, or its own depth when it already
    /// paints last. Paint order is `(z, NodeId)`, so a tie on depth puts the higher id in front.
    /// `None` when `key` has no placement.
    pub(crate) fn raised_z(&self, key: NodeId) -> Option<u32> {
        let own = self.item_placements.get(&key)?.z;
        let top_other = self
            .item_placements
            .iter()
            .filter(|(id, _)| **id != key)
            .map(|(id, placement)| (placement.z, *id))
            .max();
        Some(match top_other {
            Some((top_z, top_id)) if (top_z, top_id) > (own, key) => top_z.saturating_add(1),
            _ => own,
        })
    }

    pub(super) fn validate(&self, graph: &CoreGraph) -> Result<(), GraphViewValidationError> {
        if !self.viewport.is_valid() {
            return Err(GraphViewValidationError::InvalidViewport);
        }

        for (key, placement) in &self.item_placements {
            if !placement.pos.is_finite() {
                return Err(GraphViewValidationError::NonFinitePosition { item: *key });
            }
        }
        // A map guarantees unique keys, so counts plus reverse membership
        // prove the graph and view contain exactly the same node set.
        if self.item_placements.len() != graph.len() {
            return Err(GraphViewValidationError::NodeCount);
        }
        for node in graph.iter() {
            if !self.item_placements.contains_key(&node.id) {
                return Err(GraphViewValidationError::MissingNode { node_id: node.id });
            }
        }
        for key in &self.selected {
            if !self.item_placements.contains_key(key) {
                return Err(GraphViewValidationError::MissingSelectedItem { item: *key });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use scenarium::NodeId;

    use crate::core::document::graph_view::{GraphView, ItemPlacement};

    /// Paint order is a total order over `(z, NodeId)`, so items sharing the
    /// seed depth still come out in a fixed sequence, and `front_z` puts a
    /// raise past every one of them. Both matter for stability: equal depths
    /// are the common case until something is raised.
    #[test]
    fn paint_order_is_total_and_front_z_clears_every_item() {
        let mut view = GraphView::default();
        assert_eq!(view.front_z(), 0, "an empty view seeds at zero");

        let (low, high) = (NodeId::from_u128(1), NodeId::from_u128(2));
        view.item_placements.insert(high, ItemPlacement::default());
        view.item_placements.insert(low, ItemPlacement::default());
        // Reused across both sweeps, the way the draw pass reuses its own.
        let mut order = Vec::new();
        view.paint_order(&mut order);
        assert_eq!(
            order.iter().map(|item| item.id).collect::<Vec<_>>(),
            [low, high],
            "equal depths break by id, not by insertion"
        );

        assert_eq!(view.front_z(), 1, "one past the shared depth of zero");
        view.item_placements.get_mut(&low).unwrap().z = view.front_z();
        view.paint_order(&mut order);
        assert_eq!(
            order.iter().map(|item| item.id).collect::<Vec<_>>(),
            [high, low],
            "raising the lower id lifts it past the higher one"
        );
        assert_eq!(view.front_z(), 2, "and the next raise clears that one too");
    }
}
