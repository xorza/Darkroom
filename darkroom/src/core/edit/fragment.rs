//! [`Fragment`]: a set of nodes of one graph, with what joins them, as it moves.
//!
//! # Decisions
//!
//! - **One fragment is what Duplicate, Copy, Paste, Collapse and Expand move**: a set of nodes
//!   with the wires among them, their places and their ties, given ids and landed in a graph.
//!   Duplicate is a capture and a paste into the same graph, as Unreal's graph editors duplicate by
//!   copying and pasting the selection. Refused: the copy written out once per command, five times.

use std::collections::{BTreeSet, HashMap};
use std::mem;

use glam::Vec2;
use scenarium::{
    Binding, BindingEntry, EventPort, GraphId, InputPort, Library, Node, NodeId, OutputPort,
    PortId, SubgraphId, Subscription,
};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::core::edit::graph_intent::route_insert::RouteInsert;
use crate::core::edit::unwired;

/// A set of nodes of one graph with the wires, subscriptions and ties among them, as a duplicate
/// copies them: [`Self::capture`] reads them out of the document, [`Self::remap`] gives them
/// fresh ids, and [`Self::into_intents`] lands them as the edits that add them.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Fragment {
    /// In the order the selection lists them.
    nodes: Vec<FragmentNode>,
    /// Each wire from one node of the fragment into another, in the order the nodes' inputs
    /// were read.
    wires: Vec<DataWire>,
    /// Each subscription of a node of the fragment that it keeps, in the graph's order: to an
    /// emitter inside it, or one outside that [`IncomingWires::Keep`] keeps.
    subscriptions: Vec<Subscription>,
    /// The ties whose every wire runs between two nodes of the fragment, in id order.
    ties: Vec<FragmentTie>,
    /// Each wire through a tie of the fragment, with those ties in its route's order.
    routes: Vec<FragmentRoute>,
}

/// A node of a [`Fragment`]: its id, the node, where it sits, and the bindings its own inputs
/// start with — each constant, and each wire from outside as the capture's [`IncomingWires`]
/// left it.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct FragmentNode {
    node_id: NodeId,
    node: Node,
    pos: Vec2,
    bindings: Vec<BindingEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct FragmentTie {
    tie: TieId,
    placed: Tie,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FragmentRoute {
    wire: WireKey,
    ties: Vec<TieId>,
}

impl Fragment {
    /// The nodes of `items` in `graph` of `document`, in `items`' order, with what joins them. A
    /// wire or a subscription that reaches a node from outside goes as `incoming` says: dropped,
    /// leaving a data input's declared default as a wire removed on its own does, or kept on the
    /// outside producer or emitter. A stub is left out: it has no ports to join, and a copy of a
    /// node that cannot run has little use. A boundary node is left out: a subgraph holds one of
    /// each side, and a wire from one to a node of `items` comes from outside.
    pub(crate) fn capture(
        document: &Document,
        graph: GraphId,
        library: &Library,
        items: &BTreeSet<ItemId>,
        incoming: IncomingWires,
    ) -> Self {
        let (held, view) = (document.graphs.graph(graph), document.view(graph));
        let scope = document.graphs.scope(library, graph);
        let inside: BTreeSet<NodeId> = items
            .iter()
            .filter_map(|item| item.node())
            .filter(|&node_id| {
                held.find(node_id)
                    .is_some_and(|node| node.boundary().is_none())
            })
            .collect();
        let mut fragment = Self {
            nodes: Vec::with_capacity(inside.len()),
            wires: Vec::new(),
            subscriptions: Vec::new(),
            ties: Vec::new(),
            routes: Vec::new(),
        };
        for &node_id in &inside {
            let node = held.find(node_id).expect("`inside` holds graph nodes");
            let mut bindings = Vec::new();
            for (port, binding) in held.input_bindings(node_id) {
                let binding = match binding {
                    Binding::Wire(source) if inside.contains(&source.node_id) => {
                        fragment.wires.push(DataWire {
                            output: *source,
                            input: port,
                        });
                        continue;
                    }
                    Binding::Wire(source) => match incoming {
                        IncomingWires::Keep => Some(Binding::Wire(*source)),
                        IncomingWires::Drop => unwired::binding(held, scope, port),
                    },
                    Binding::Const(_) => Some(binding.clone()),
                };
                bindings.extend(binding.map(|binding| BindingEntry { port, binding }));
            }
            fragment.nodes.push(FragmentNode {
                node_id,
                node: node.clone(),
                pos: view
                    .node_placements
                    .get(&node_id)
                    .expect("the view places every node the graph holds")
                    .pos,
                bindings,
            });
        }
        fragment.subscriptions = held
            .subscriptions()
            .filter(|subscription| inside.contains(&subscription.subscriber))
            .filter(|subscription| {
                incoming == IncomingWires::Keep || inside.contains(&subscription.event.node_id)
            })
            .collect();
        fragment.ties = view
            .ties_within(|node_id| inside.contains(&node_id))
            .into_iter()
            .map(|tie| FragmentTie {
                tie,
                placed: view.ties[&tie],
            })
            .collect();
        let captured: BTreeSet<TieId> = fragment.ties.iter().map(|tie| tie.tie).collect();
        // A tie that also holds a wire leaving the set is not captured, so its step drops out.
        fragment.routes = view
            .routes
            .iter()
            .filter_map(|(&wire, route)| {
                let ties: Vec<TieId> = route
                    .iter()
                    .copied()
                    .filter(|tie| captured.contains(tie))
                    .collect();
                (!ties.is_empty()).then_some(FragmentRoute { wire, ties })
            })
            .collect();
        fragment
    }

    /// The subgraphs its instances place, each once, in its node order.
    pub(crate) fn placed(&self) -> impl Iterator<Item = SubgraphId> + '_ {
        let mut seen = BTreeSet::new();
        self.nodes
            .iter()
            .filter_map(|node| node.node.subgraph())
            .filter(move |&id| seen.insert(id))
    }

    /// Point each instance of the fragment at `to`'s answer for the subgraph it places, as a
    /// paste does with a subgraph that comes in under a fresh id.
    pub(crate) fn repoint(&mut self, to: impl Fn(SubgraphId) -> SubgraphId) {
        for fragment_node in &mut self.nodes {
            if let Some(placed) = fragment_node.node.subgraph() {
                fragment_node.node.set_subgraph(to(placed));
            }
        }
    }

    /// The ids of its nodes, in its order: the order [`Self::remap`] takes fresh ids in.
    pub(crate) fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().map(|node| node.node_id)
    }

    /// Put `to`'s answer for each port of `node`, a node outside the fragment, in place of each
    /// wire a node of it reads from that port; an input `to` answers nothing for is left unbound.
    /// What an expand does with the wires from a subgraph's `Inputs` node: each takes what the
    /// instance's own input held.
    pub(crate) fn rebind_from(&mut self, node: NodeId, to: impl Fn(PortId) -> Option<Binding>) {
        for fragment_node in &mut self.nodes {
            fragment_node.bindings = mem::take(&mut fragment_node.bindings)
                .into_iter()
                .filter_map(|entry| match entry.binding {
                    Binding::Wire(source) if source.node_id == node => {
                        to(source.port).map(|binding| BindingEntry {
                            port: entry.port,
                            binding,
                        })
                    }
                    _ => Some(entry),
                })
                .collect();
        }
    }

    /// This fragment under fresh ids from `fresh`, one per node in order, then one per tie: every
    /// reference among its nodes and ties follows, and a reference to an outside producer or
    /// emitter stays.
    pub(crate) fn remap(self, mut fresh: impl FnMut() -> u128) -> Self {
        let nodes: HashMap<NodeId, NodeId> = self
            .nodes
            .iter()
            .map(|node| (node.node_id, NodeId::from_u128(fresh())))
            .collect();
        let ties: HashMap<TieId, TieId> = self
            .ties
            .iter()
            .map(|tie| (tie.tie, TieId::from_u128(fresh())))
            .collect();
        let node = |node_id: NodeId| nodes.get(&node_id).copied().unwrap_or(node_id);
        let input = |port: InputPort| InputPort::new(node(port.node_id), port.port);
        let output = |port: OutputPort| OutputPort::new(node(port.node_id), port.port);
        let subscription = |subscription: Subscription| {
            Subscription::new(
                EventPort::new(node(subscription.event.node_id), subscription.event.event),
                node(subscription.subscriber),
            )
        };
        let wire = |wire: DataWire| DataWire {
            output: output(wire.output),
            input: input(wire.input),
        };
        Self {
            nodes: self
                .nodes
                .into_iter()
                .map(|fragment_node| FragmentNode {
                    node_id: node(fragment_node.node_id),
                    bindings: fragment_node
                        .bindings
                        .into_iter()
                        .map(|entry| BindingEntry {
                            port: input(entry.port),
                            binding: entry.binding,
                        })
                        .collect(),
                    ..fragment_node
                })
                .collect(),
            wires: self.wires.into_iter().map(wire).collect(),
            subscriptions: self.subscriptions.into_iter().map(subscription).collect(),
            ties: self
                .ties
                .into_iter()
                .map(|tie| FragmentTie {
                    tie: ties[&tie.tie],
                    ..tie
                })
                .collect(),
            routes: self
                .routes
                .into_iter()
                .map(|route| FragmentRoute {
                    wire: match route.wire {
                        WireKey::Data(data) => WireKey::Data(wire(data)),
                        WireKey::Event(event) => WireKey::Event(subscription(event)),
                    },
                    ties: route.ties.iter().map(|tie| ties[tie]).collect(),
                })
                .collect(),
        }
    }

    /// The edits that add this fragment `offset` from where it sits, and select it: each node with
    /// its bindings, then the wires among them once every node exists, the subscriptions, the
    /// ties with their routes, and the selection. Empty for a fragment of no node.
    pub(crate) fn into_intents(self, offset: Vec2) -> Vec<GraphIntent> {
        if self.nodes.is_empty() {
            return Vec::new();
        }
        let selection: BTreeSet<ItemId> = self
            .nodes
            .iter()
            .map(|node| ItemId::Node(node.node_id))
            .chain(self.ties.iter().map(|tie| ItemId::Tie(tie.tie)))
            .collect();
        let mut intents: Vec<GraphIntent> = self
            .nodes
            .into_iter()
            .map(|node| GraphIntent::AddNode {
                pos: node.pos + offset,
                node_id: node.node_id,
                node: node.node,
                bindings: node.bindings,
            })
            .collect();
        intents.extend(self.wires.into_iter().map(|wire| GraphIntent::SetInput {
            input: wire.input,
            to: Some(Binding::Wire(wire.output)),
        }));
        intents.extend(self.subscriptions.into_iter().map(|subscription| {
            GraphIntent::SetSubscription {
                subscription,
                to: true,
            }
        }));
        let mut made = BTreeSet::new();
        for FragmentTie { tie, placed } in self.ties {
            // A tie joins each route behind the ties already made on it.
            let wires = self
                .routes
                .iter()
                .filter_map(|route| {
                    let idx = route.ties.iter().position(|&step| step == tie)?;
                    Some(RouteInsert {
                        wire: route.wire,
                        at: route.ties[..idx]
                            .iter()
                            .filter(|step| made.contains(*step))
                            .count(),
                    })
                })
                .collect();
            intents.push(GraphIntent::TieWires {
                tie,
                pos: placed.pos + offset,
                wires,
            });
            if let TieAngle::Pinned(_) = placed.angle {
                intents.push(GraphIntent::SetTieAngle {
                    tie,
                    angle: placed.angle,
                    gesture: None,
                });
            }
            made.insert(tie);
        }
        intents.push(GraphIntent::SetSelection { to: selection });
        intents
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use glam::Vec2;
    use scenarium::{Binding, GraphId, InputPort, NodeId, OutputPort, numbered_port};

    use crate::core::document::internals::DocumentFixture;
    use crate::core::document::item_id::ItemId;
    use crate::core::document::tie::TieId;
    use crate::core::document::wire_key::{DataWire, WireKey};
    use crate::core::edit::fragment::Fragment;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
    use crate::core::edit::graph_intent::route_insert::RouteInsert;

    /// Two wired nodes and the tie on their wire, captured, remapped and landed `offset` away:
    /// the fresh ids go to the nodes in selection order, then to the tie, so the edits add each
    /// node under its new id where it sat plus the offset, wire the new `a` into the new `b`, tie
    /// that wire at the tie's place plus the offset, and select the three.
    #[test]
    fn a_fragment_lands_its_wire_and_tie_between_the_new_ids() {
        let mut fixture = DocumentFixture::default();
        let (a_at, b_at, tie_at) = (Vec2::ZERO, Vec2::new(200.0, 0.0), Vec2::new(100.0, 10.0));
        let a = fixture.node_at(a_at);
        let b = fixture.node_at(b_at);
        let wire = fixture.wire(
            OutputPort::new(a, numbered_port(1)),
            InputPort::new(b, numbered_port(1)),
        );
        fixture.tie(tie_at, [wire]);
        let selected = BTreeSet::from([ItemId::Node(a), ItemId::Node(b)]);
        let mut next = 10;
        let fragment = Fragment::capture(
            &fixture.document,
            GraphId::Main,
            &fixture.library,
            &selected,
            IncomingWires::Drop,
        )
        .remap(|| {
            next += 1;
            next
        });

        // Selection order is id order; the tie takes the id after both nodes'.
        let new = |old: NodeId| {
            let (first, _) = if a < b { (a, b) } else { (b, a) };
            NodeId::from_u128(if old == first { 11 } else { 12 })
        };
        let (new_a, new_b, new_tie) = (new(a), new(b), TieId::from_u128(13));
        let offset = Vec2::new(32.0, 32.0);
        let cloned = DataWire {
            output: OutputPort::new(new_a, numbered_port(1)),
            input: InputPort::new(new_b, numbered_port(1)),
        };
        let intents = fragment.into_intents(offset);
        let added: Vec<(NodeId, Vec2)> = intents
            .iter()
            .filter_map(|intent| match intent {
                GraphIntent::AddNode { node_id, pos, .. } => Some((*node_id, *pos)),
                _ => None,
            })
            .collect();
        let mut expected = vec![(new_a, a_at + offset), (new_b, b_at + offset)];
        expected.sort_unstable_by_key(|(node_id, _)| *node_id);
        assert_eq!(added, expected);
        assert!(matches!(
            &intents[2..],
            [
                GraphIntent::SetInput {
                    input,
                    to: Some(Binding::Wire(output)),
                },
                GraphIntent::TieWires { tie, pos, wires },
                GraphIntent::SetSelection { to },
            ] if *input == cloned.input
                && *output == cloned.output
                && *tie == new_tie
                && *pos == tie_at + offset
                && *wires == [RouteInsert { wire: WireKey::Data(cloned), at: 0 }]
                && *to == BTreeSet::from([
                    ItemId::Node(new_a),
                    ItemId::Node(new_b),
                    ItemId::Tie(new_tie),
                ])
        ));
    }
}
