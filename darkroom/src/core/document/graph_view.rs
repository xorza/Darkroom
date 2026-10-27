//! [`GraphView`]: how the editor places, stacks and frames the graph.

use std::collections::{BTreeMap, BTreeSet};

use glam::Vec2;
use scenarium::{DetachedNode, Graph as CoreGraph, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::error::GraphViewValidationError;
use crate::core::document::item_id::ItemId;
use crate::core::document::route::Route;
use crate::core::document::stubs::Stubs;
use crate::core::document::tie::{Tie, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::document::wire_key::WireKey;

/// Where one node body sits and how far forward it paints.
///
/// The two are independent: dragging a node changes `pos` and nothing else,
/// raising it changes `z` and nothing else. Keeping the stacking order as a
/// *value* here rather than as the position of an entry in an ordered map is
/// what lets [`GraphView`] derive its equality and serde, and what makes
/// [`SetDepth`](crate::core::edit::step::set_depth::SetDepth) a field write instead of a reorder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct NodePlacement {
    /// Canvas-world position of the body's top-left corner.
    pub(crate) pos: Vec2,
    /// Paint depth: higher paints later, so it sits in front. Equal depths
    /// break by `NodeId`, so the order is total and stable across frames — see
    /// [`GraphView::paint_order`].
    pub(crate) z: u32,
}

/// One entry of a [`GraphView::paint_order`] sweep, sorted by its own derived
/// order — depth first, then id.
///
/// The break on `id` is what makes the order *total*: several nodes share
/// the seed `z` of 0 until something raises them, and a comparator that let
/// those be equal would leave their relative order at the mercy of the sort's
/// stability. Carrying `z` alongside the id is what keeps the sort from
/// descending [`GraphView::node_placements`] once per comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StackedNode {
    /// Paint depth: higher paints later, so it sits in front.
    pub(crate) z: u32,
    pub(crate) node_id: NodeId,
}

/// Editor-side view metadata for the document's graph: per-node placements,
/// the wire ties and the routes through them, the viewport, and the
/// selection (`Document::main_view`). The graph *data*
/// itself lives in the core `Graph`; this is purely how the editor presents
/// and navigates it.
///
/// Everything here is saved, so reopening a file restores the exact camera
/// and selection. Ctrl+Z walks the selection with the structural edits but
/// not the camera (`darkroom/AGENTS.md`, "The camera is outside the undo history").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GraphView {
    /// Every node body's placement. Exactly one entry per graph node and per
    /// stub — enforced by [`Self::validate`].
    ///
    /// Order-insensitive on purpose: paint order rides in each entry's
    /// [`NodePlacement::z`], so this map is free to be keyed and compared as a
    /// set. A caller that needs the stacking asks for [`Self::paint_order`].
    pub(crate) node_placements: BTreeMap<NodeId, NodePlacement>,
    /// Every wire tie. Each holds at least one wire, a route that names it.
    pub(crate) ties: BTreeMap<TieId, Tie>,
    /// The route of each tied wire the graph holds, keyed by both of the
    /// wire's ends; an untied wire has none.
    pub(crate) routes: BTreeMap<WireKey, Route>,
    pub(crate) viewport: Viewport,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering).
    pub(crate) selected: BTreeSet<ItemId>,
}

impl GraphView {
    /// A fresh view over `graph`: one zero-positioned, unraised placement per
    /// node, no ties, the default camera, nothing selected.
    ///
    /// The only way a view is built from a graph, so there is nowhere for the
    /// two to disagree about which nodes are placed.
    pub(crate) fn new(graph: &CoreGraph) -> Self {
        Self {
            node_placements: graph
                .iter()
                .map(|node| (node.node_id, NodePlacement::default()))
                .collect(),
            ..Self::default()
        }
    }

    /// The nodes back-to-front into `out`: what a paint pass draws in order,
    /// and the only place stacking is resolved. `out` is cleared first, so a
    /// caller hands the same buffer over every frame.
    ///
    /// Fills a caller-owned buffer rather than returning one because the
    /// production caller is the per-frame node draw: a fresh `Vec` here would
    /// be one allocation per graph pane per frame, sized to the whole graph
    /// and taken before culling drops the off-screen nodes.
    pub(crate) fn paint_order(&self, out: &mut Vec<StackedNode>) {
        out.clear();
        out.reserve_exact(self.node_placements.len());
        out.extend(
            self.node_placements
                .iter()
                .map(|(id, placement)| StackedNode {
                    z: placement.z,
                    node_id: *id,
                }),
        );
        out.sort_unstable();
    }

    /// The depth that puts a node in front of every other. One past the
    /// current maximum, so a raise never has to renumber its neighbours.
    pub(crate) fn front_z(&self) -> u32 {
        self.node_placements
            .values()
            .map(|placement| placement.z)
            .max()
            .map_or(0, |top| top.saturating_add(1))
    }

    /// The depth that puts `node_id` in front of every *other* node, or its own depth when it already
    /// paints last. Paint order is `(z, NodeId)`, so of two equal depths the higher id is in front.
    /// `None` when `node_id` has no placement.
    pub(crate) fn raised_z(&self, node_id: NodeId) -> Option<u32> {
        let own = self.node_placements.get(&node_id)?.z;
        let top_other = self
            .node_placements
            .iter()
            .filter(|(id, _)| **id != node_id)
            .map(|(id, placement)| (placement.z, *id))
            .max();
        Some(match top_other {
            Some((top_z, top_id)) if (top_z, top_id) > (own, node_id) => top_z.saturating_add(1),
            _ => own,
        })
    }

    pub(super) fn validate(
        &self,
        graph: &CoreGraph,
        stubs: &Stubs,
    ) -> Result<(), GraphViewValidationError> {
        if !self.viewport.is_valid() {
            return Err(GraphViewValidationError::InvalidViewport);
        }

        for (node_id, placement) in &self.node_placements {
            if !placement.pos.is_finite() {
                return Err(GraphViewValidationError::NonFinitePos { node_id: *node_id });
            }
        }
        // A map guarantees unique keys, and the graph and stubs are disjoint,
        // so counts plus reverse membership prove the view places exactly
        // their node set.
        if self.node_placements.len() != graph.len() + stubs.len() {
            return Err(GraphViewValidationError::NodeCount);
        }
        let ids = graph
            .iter()
            .map(|node| node.node_id)
            .chain(stubs.iter().map(DetachedNode::node_id));
        for node_id in ids {
            if !self.node_placements.contains_key(&node_id) {
                return Err(GraphViewValidationError::MissingNode { node_id });
            }
        }
        self.validate_ties(graph)?;
        if let Some(&item) = self.selected.iter().find(|&&item| !self.holds(item)) {
            return Err(GraphViewValidationError::MissingSelectedItem { item });
        }
        Ok(())
    }

    /// Check that each route names a wire `graph` holds and keeps the route's
    /// rules, each step names a tie, and each tie holds a wire and is placed
    /// where a canvas can draw it.
    fn validate_ties(&self, graph: &CoreGraph) -> Result<(), GraphViewValidationError> {
        for (&wire, route) in &self.routes {
            if !wire.is_held_by(graph) {
                return Err(GraphViewValidationError::RouteToNoWire { wire });
            }
            route
                .check()
                .map_err(|source| GraphViewValidationError::Route { wire, source })?;
            if let Some(&tie) = route.iter().find(|tie| !self.ties.contains_key(tie)) {
                return Err(GraphViewValidationError::RouteToNoTie { wire, tie });
            }
        }
        for (&tie, placed) in &self.ties {
            if !self.routes.values().flatten().any(|&routed| routed == tie) {
                return Err(GraphViewValidationError::UnroutedTie { tie });
            }
            if !placed.pos.is_finite() {
                return Err(GraphViewValidationError::NonFiniteTiePos { tie });
            }
            if !placed.angle.is_valid() {
                return Err(GraphViewValidationError::InvalidTieAngle { tie });
            }
        }
        Ok(())
    }

    /// Each tie whose every wire has both end nodes among those `holds`
    /// answers for, in id order: the ties a drag or a duplicate of those nodes
    /// takes with them. `darkroom/AGENTS.md`, "Wire ties", "A node set takes the ties
    /// inside it", holds the decision.
    pub(crate) fn ties_within(&self, holds: impl Fn(NodeId) -> bool) -> Vec<TieId> {
        // Every tie holds a wire, so a tie no outside wire names has only
        // inside ones.
        let outside: BTreeSet<TieId> = self
            .routes
            .iter()
            .filter(|(wire, _)| !wire.ends().into_iter().all(&holds))
            .flat_map(|(_, route)| route.iter().copied())
            .collect();
        self.ties
            .keys()
            .copied()
            .filter(|tie| !outside.contains(tie))
            .collect()
    }

    /// Where `item` sits, the top-left corner of a node body or a tie's
    /// center, or `None` for an item the view does not hold.
    pub(crate) fn pos(&self, item: ItemId) -> Option<Vec2> {
        match item {
            ItemId::Node(node_id) => self.node_placements.get(&node_id).map(|placed| placed.pos),
            ItemId::Tie(tie) => self.ties.get(&tie).map(|placed| placed.pos),
        }
    }

    /// Whether the view holds `item`: places the node, or holds the tie.
    pub(crate) fn holds(&self, item: ItemId) -> bool {
        match item {
            ItemId::Node(node_id) => self.node_placements.contains_key(&node_id),
            ItemId::Tie(tie) => self.ties.contains_key(&tie),
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use scenarium::{EventPort, InputPort, NodeId, OutputPort};

    use crate::core::document::error::{GraphViewValidationError, RouteError};
    use crate::core::document::graph_view::{GraphView, NodePlacement};
    use crate::core::document::internals::DocumentFixture;
    use crate::core::document::item_id::ItemId;
    use crate::core::document::route::Route;
    use crate::core::document::tie::{TieAngle, TieId};
    use crate::core::document::wire_key::{DataWire, WireKey};

    /// What a case names, how it breaks a valid view, and the error it gives.
    type Fault<'a> = (
        &'a str,
        &'a dyn Fn(&mut GraphView),
        GraphViewValidationError,
    );

    /// A tie on a data wire and an event wire, selected, passes; each rule
    /// broken alone fails with its own case. A route keyed by the input with
    /// another output names no wire, so the key's output matters; a pinned
    /// angle passes at 0 and just under 360, and fails at 360 and at NaN.
    #[test]
    fn validation_holds_routes_and_ties_to_the_graph() {
        let mut fixture = DocumentFixture::default();
        let [a, b] = [0.0, 220.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
        let data = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 0));
        let event = fixture.subscribe(EventPort::new(a, 0), b);
        let tie = fixture.tie(Vec2::new(110.0, 0.0), [data, event]);
        let DocumentFixture { mut document, .. } = fixture;
        document.main_view.selected.insert(ItemId::Tie(tie));
        let (graph, stubs) = (&document.graph, &document.stubs);
        let valid = document.main_view.clone();
        valid.validate(graph, stubs).unwrap();

        let ghost = TieId::unique();
        let other_output = WireKey::Data(DataWire {
            output: OutputPort::new(b, 0),
            input: InputPort::new(b, 0),
        });
        let with = |change: &dyn Fn(&mut GraphView)| {
            let mut view = valid.clone();
            change(&mut view);
            view.validate(graph, stubs)
        };
        let route = |ties: &[TieId]| Route::unchecked(ties.iter().copied());
        for degrees in [0.0, 359.9] {
            with(&|view| view.ties.get_mut(&tie).unwrap().angle = TieAngle::Pinned(degrees))
                .unwrap();
        }
        let cases: [Fault<'_>; 9] = [
            (
                "a route keyed by the input and another output",
                &|view| {
                    view.routes.insert(other_output, route(&[tie]));
                },
                GraphViewValidationError::RouteToNoWire { wire: other_output },
            ),
            (
                "an empty route",
                &|view| {
                    view.routes.insert(data, route(&[]));
                },
                GraphViewValidationError::Route {
                    wire: data,
                    source: RouteError::Empty,
                },
            ),
            (
                "a route through one tie twice",
                &|view| {
                    view.routes.insert(data, route(&[tie, tie]));
                },
                GraphViewValidationError::Route {
                    wire: data,
                    source: RouteError::RepeatedTie { tie },
                },
            ),
            (
                "a step to no tie",
                &|view| {
                    view.routes.insert(data, route(&[tie, ghost]));
                },
                GraphViewValidationError::RouteToNoTie {
                    wire: data,
                    tie: ghost,
                },
            ),
            (
                "a tie with no wire",
                &|view| {
                    let copy = view.ties[&tie];
                    view.ties.insert(ghost, copy);
                },
                GraphViewValidationError::UnroutedTie { tie: ghost },
            ),
            (
                "a tie at no finite position",
                &|view| view.ties.get_mut(&tie).unwrap().pos.x = f32::INFINITY,
                GraphViewValidationError::NonFiniteTiePos { tie },
            ),
            (
                "a tie pinned to 360",
                &|view| view.ties.get_mut(&tie).unwrap().angle = TieAngle::Pinned(360.0),
                GraphViewValidationError::InvalidTieAngle { tie },
            ),
            (
                "a tie pinned to NaN",
                &|view| view.ties.get_mut(&tie).unwrap().angle = TieAngle::Pinned(f32::NAN),
                GraphViewValidationError::InvalidTieAngle { tie },
            ),
            (
                "a selected tie the view does not hold",
                &|view| {
                    view.selected.insert(ItemId::Tie(ghost));
                },
                GraphViewValidationError::MissingSelectedItem {
                    item: ItemId::Tie(ghost),
                },
            ),
        ];
        for (what, change, expected) in cases {
            assert_eq!(with(change), Err(expected), "{what}");
        }
    }

    /// Paint order is a total order over `(z, NodeId)`, so nodes sharing the
    /// seed depth still come out in a fixed sequence, and `front_z` puts a
    /// raise past every one of them. Both matter for stability: equal depths
    /// are the common case until something is raised.
    #[test]
    fn paint_order_is_total_and_front_z_clears_every_item() {
        let mut view = GraphView::default();
        assert_eq!(view.front_z(), 0, "an empty view seeds at zero");

        let (low, high) = (NodeId::from_u128(1), NodeId::from_u128(2));
        view.node_placements.insert(high, NodePlacement::default());
        view.node_placements.insert(low, NodePlacement::default());
        // Reused across both sweeps, the way the draw pass reuses its own.
        let mut order = Vec::new();
        view.paint_order(&mut order);
        assert_eq!(
            order
                .iter()
                .map(|node_id| node_id.node_id)
                .collect::<Vec<_>>(),
            [low, high],
            "equal depths break by id, not by insertion"
        );

        assert_eq!(view.front_z(), 1, "one past the shared depth of zero");
        view.node_placements.get_mut(&low).unwrap().z = view.front_z();
        view.paint_order(&mut order);
        assert_eq!(
            order
                .iter()
                .map(|node_id| node_id.node_id)
                .collect::<Vec<_>>(),
            [high, low],
            "raising the lower id lifts it past the higher one"
        );
        assert_eq!(view.front_z(), 2, "and the next raise clears that one too");
    }
}
