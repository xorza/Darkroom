//! Collapse and Expand: a selection turned into a new compound and one node of it, and one
//! instance's nodes put back in its graph.
//!
//! - **Collapse and Expand are each a plan lowered to ordinary requests**: a new definition,
//!   graph edits inside it and in the graph around it, raised in one batch so they land as one
//!   history entry. As Blender's Make Group and Ungroup. Refused: a step of their own, which
//!   would repeat what adding, removing and wiring nodes already record.
//! - **One port per source a crossing wire reads, of the source's type**, and one per inside
//!   output an outside consumer reads: several inputs that share a source share its port, and
//!   the source's type is one each of them admitted.
//! - **A collapse is refused when the selection would feed itself**: a wire leaves it and a later
//!   one returns, through a node the refusal names. Refused: collapsing anyway, which leaves a
//!   graph no compile accepts.
//! - **A collapse keeps the nodes' ids** inside the new definition, a graph of its own; an expand
//!   mints fresh ones, as its nodes join a graph that may hold them already.
//! - **Collapse and Expand are inverses**, as Blender's Make Group and Ungroup and ComfyUI's
//!   convert and unpack.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use glam::Vec2;
use scenarium::{
    Binding, Compound, CompoundId, GraphId, InputDecl, InputPort, Interface, Library, Node, NodeId,
    Numbered, OutputDecl, OutputPort, PortId, Ports, TypeNames, Wiring,
};
use thiserror::Error;

use crate::core::document::Document;
use crate::core::document::compound_side::CompoundSide;
use crate::core::document::compound_side::ReadOnly;
use crate::core::document::item_id::ItemId;
use crate::core::edit::compound_intent::{CompoundIntent, NewCompound};
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::error::EditRefusal;
use crate::core::edit::fragment::Fragment;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;

/// How far left of the selection the new definition's `Inputs` node sits, and right its
/// `Outputs`, in canvas units.
const BOUNDARY_GAP: f32 = 240.0;

/// Why a selection does not collapse.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum CollapseRefusal {
    /// A wire leaves the selection and a later one returns through `name`: the new instance
    /// would feed itself.
    #[error("the selection would feed itself through {name:?}")]
    FeedsItself { node: NodeId, name: String },
    /// An event subscription joins a node of the selection and one outside it, which a collapse,
    /// making data ports alone, does not carry.
    #[error("{name:?} is subscribed across the selection's edge")]
    SubscriptionCrosses { node: NodeId, name: String },
    /// The graph follows the library or is locked: a collapse would add the new compound and
    /// have every edit of the graph refused.
    #[error("{}", EditRefusal::ReadOnly { why: *why })]
    ReadOnly { why: ReadOnly },
}

/// What a collapse raises, in order: the new definition, the edits that fill it, and the edits
/// of the graph around the selection.
#[derive(Debug)]
pub(crate) struct Collapse {
    pub(crate) compound: NewCompound,
    /// Edits of the new definition's graph.
    pub(crate) inside: Vec<GraphIntent>,
    /// Edits of the graph the selection is in.
    pub(crate) outside: Vec<GraphIntent>,
}

impl Collapse {
    /// Queue this collapse of `graph` on `queue`, in the order it lands: one batch, so one
    /// history entry.
    pub(crate) fn queue(self, graph: GraphId, queue: &mut DocumentQueue) {
        let inner = GraphId::Compound(self.compound.id);
        queue.push_compound(CompoundIntent::Add(Box::new(self.compound)));
        queue.extend_graph(inner, self.inside);
        queue.extend_graph(graph, self.outside);
    }
}

/// One port of the new interface: where it reads from or feeds, and the ports on the other side.
#[derive(Debug)]
struct Crossing {
    /// The output the crossing wires read.
    source: OutputPort,
    /// The inputs they feed.
    consumers: Vec<InputPort>,
}

/// Plan collapsing the graph nodes of `selection` in `graph` of `document` into a new compound
/// and one instance of it; `None` when the selection holds no graph node. A boundary node and a
/// ghost node in the selection stay where they are.
pub(crate) fn collapse(
    document: &Document,
    graph: GraphId,
    library: &Library,
    selection: &BTreeSet<ItemId>,
) -> Option<Result<Collapse, CollapseRefusal>> {
    if let Some(why) = document.read_only(graph) {
        return Some(Err(CollapseRefusal::ReadOnly { why }));
    }
    let held = document.graphs.graph(graph);
    let inside: BTreeSet<NodeId> = selection
        .iter()
        .filter_map(|item| item.node())
        .filter(|&node_id| {
            held.find(node_id)
                .is_some_and(|node| node.boundary().is_none())
        })
        .collect();
    if inside.is_empty() {
        return None;
    }
    let scope = document.graphs.scope(library, graph);
    let name = |node_id: NodeId| {
        scope
            .title(held.find(node_id).expect("a crossing names a graph node"))
            .to_owned()
    };
    for subscription in held.wiring().subscriptions() {
        let (emitter, subscriber) = (subscription.event.node_id, subscription.trigger.node_id);
        if inside.contains(&emitter) != inside.contains(&subscriber) {
            let node = if inside.contains(&emitter) {
                subscriber
            } else {
                emitter
            };
            return Some(Err(CollapseRefusal::SubscriptionCrosses {
                node,
                name: name(node),
            }));
        }
    }
    if let Some(node) = feeds_itself(document, graph, &inside) {
        return Some(Err(CollapseRefusal::FeedsItself {
            node,
            name: name(node),
        }));
    }

    // The crossing wires, grouped by the output they read, in the graph's binding order.
    let mut incoming: Vec<Crossing> = Vec::new();
    let mut outgoing: Vec<Crossing> = Vec::new();
    for (input, binding) in held.wiring().bindings() {
        let Binding::Wire(source) = *binding else {
            continue;
        };
        let crossing = match (
            inside.contains(&source.node_id),
            inside.contains(&input.node_id),
        ) {
            (false, true) => &mut incoming,
            (true, false) => &mut outgoing,
            _ => continue,
        };
        match crossing
            .iter_mut()
            .find(|crossing| crossing.source == source)
        {
            Some(crossing) => crossing.consumers.push(input),
            None => crossing.push(Crossing {
                source,
                consumers: vec![input],
            }),
        }
    }

    let scope = document.graphs.scope(library, graph);
    let output_decl = |port: OutputPort| -> &OutputDecl {
        scope
            .decl(held.find(port.node_id).expect("a wire reads a graph node"))
            .ports()
            .output(port.port)
            .expect("a wire reads a declared output")
    };
    let input_name = |port: InputPort| -> String {
        scope
            .decl(held.find(port.node_id).expect("a wire feeds a graph node"))
            .ports()
            .input(port.port)
            .expect("a wire feeds a declared input")
            .name
            .clone()
    };
    let port_id = |idx: usize| PortId::new(idx as u32 + 1);
    let interface = Interface::new(Ports {
        inputs: incoming
            .iter()
            .enumerate()
            .map(|(idx, crossing)| {
                Numbered::new(
                    port_id(idx),
                    InputDecl::optional(
                        input_name(crossing.consumers[0]),
                        output_decl(crossing.source).data_type,
                    ),
                )
            })
            .collect(),
        outputs: outgoing
            .iter()
            .enumerate()
            .map(|(idx, crossing)| {
                let decl = output_decl(crossing.source);
                Numbered::new(
                    port_id(idx),
                    OutputDecl::new(decl.name.clone(), decl.data_type),
                )
            })
            .collect(),
    });

    let view = document.view(graph);
    let poss: Vec<Vec2> = inside
        .iter()
        .map(|node_id| view.node_placements[node_id].pos)
        .collect();
    let min = poss.iter().copied().fold(Vec2::INFINITY, Vec2::min);
    let max = poss.iter().copied().fold(Vec2::NEG_INFINITY, Vec2::max);
    let centroid = poss.iter().copied().sum::<Vec2>() / poss.len() as f32;

    let id = CompoundId::unique();
    let compound = Compound::new(document.free_compound_name("Compound"), interface);
    let inputs_node = compound.inputs_node();
    let outputs_node = compound.outputs_node();
    let mut side = CompoundSide::new(&compound.graph);
    for (node_id, x) in [
        (inputs_node, min.x - BOUNDARY_GAP),
        (outputs_node, max.x + BOUNDARY_GAP),
    ] {
        side.view
            .node_placements
            .get_mut(&node_id)
            .expect("a new definition places its boundary nodes")
            .pos = Vec2::new(x, centroid.y);
    }
    let instance = Node::instance(id);

    let items: BTreeSet<ItemId> = inside.iter().copied().map(ItemId::Node).collect();
    let fragment = Fragment::capture(document, graph, library, &items, IncomingWires::Drop);
    let mut inside_intents = fragment.into_intents(Vec2::ZERO);
    for (idx, crossing) in incoming.iter().enumerate() {
        for &consumer in &crossing.consumers {
            inside_intents.push(GraphIntent::SetInput {
                input: consumer,
                to: Some(Binding::Wire(OutputPort::new(inputs_node, port_id(idx)))),
            });
        }
    }
    for (idx, crossing) in outgoing.iter().enumerate() {
        inside_intents.push(GraphIntent::SetInput {
            input: InputPort::new(outputs_node, port_id(idx)),
            to: Some(Binding::Wire(crossing.source)),
        });
    }

    let instance_id = NodeId::unique();
    let mut outside: Vec<GraphIntent> = inside
        .iter()
        .map(|&node_id| GraphIntent::RemoveNode { node_id })
        .collect();
    outside.push(GraphIntent::AddNode {
        pos: centroid,
        node_id: instance_id,
        node: instance,
        bindings: {
            let mut bindings = Wiring::default();
            for (idx, crossing) in incoming.iter().enumerate() {
                bindings.bind(
                    InputPort::new(instance_id, port_id(idx)),
                    Binding::Wire(crossing.source),
                );
            }
            bindings
        },
    });
    for (idx, crossing) in outgoing.iter().enumerate() {
        for &consumer in &crossing.consumers {
            outside.push(GraphIntent::SetInput {
                input: consumer,
                to: Some(Binding::Wire(OutputPort::new(instance_id, port_id(idx)))),
            });
        }
    }
    outside.push(GraphIntent::SetSelection {
        to: BTreeSet::from([ItemId::Node(instance_id)]),
    });

    Some(Ok(Collapse {
        compound: NewCompound {
            id,
            compound,
            side,
            type_names: TypeNames::default(),
        },
        inside: inside_intents,
        outside,
    }))
}

/// The edits of `graph` that put the nodes of `instance`'s compound back in its place, under
/// fresh ids, wired as the instance was: each input the instance's own binding fed takes that
/// binding, and each consumer of an instance output reads what fed the `Outputs` node there. The
/// boundary nodes stay behind, and the definition stays in the document.
///
/// # Panics
/// If `graph` holds no instance `instance`.
pub(crate) fn expand(
    document: &Document,
    graph: GraphId,
    library: &Library,
    instance: NodeId,
) -> Vec<GraphIntent> {
    let held = document.graphs.graph(graph);
    let compound_id = held
        .find(instance)
        .and_then(Node::compound)
        .expect("an expand names an instance");
    let inner = GraphId::Compound(compound_id);
    let definition = document
        .graphs
        .compound(compound_id)
        .expect("an instance's compound is in the set");
    let outputs_node = definition.outputs_node();
    let items: BTreeSet<ItemId> = definition
        .graph
        .iter()
        .filter(|node| node.boundary().is_none())
        .map(|node| ItemId::Node(node.node_id))
        .collect();
    if items.is_empty() {
        return vec![GraphIntent::RemoveNode { node_id: instance }];
    }
    let mut fragment = Fragment::capture(document, inner, library, &items, IncomingWires::Keep);
    for inputs_node in definition.inputs_nodes() {
        fragment.rebind_from(inputs_node, |port: PortId| {
            held.wiring()
                .binding(InputPort::new(instance, port))
                .cloned()
        });
    }
    // Fresh ids in the fragment's node order, so the outputs below follow the nodes.
    let fresh: HashMap<NodeId, NodeId> = fragment
        .node_ids()
        .map(|node_id| (node_id, NodeId::unique()))
        .collect();
    let order: VecDeque<u128> = fragment
        .node_ids()
        .map(|node_id| fresh[&node_id].as_u128())
        .collect();
    let mut order = order;
    let fragment = fragment.remap(|| {
        order
            .pop_front()
            .unwrap_or_else(|| NodeId::unique().as_u128())
    });
    let inner_view = document.view(inner);
    let poss: Vec<Vec2> = items
        .iter()
        .filter_map(|item| item.node())
        .map(|node_id| inner_view.node_placements[&node_id].pos)
        .collect();
    let centroid = poss.iter().copied().sum::<Vec2>() / poss.len() as f32;
    let at = document.view(graph).node_placements[&instance].pos;
    let mut intents = fragment.into_intents(at - centroid);
    // The selection the fragment raises comes last; keep it so.
    let selection = intents.pop();
    intents.insert(0, GraphIntent::RemoveNode { node_id: instance });
    let fed: BTreeMap<PortId, Binding> = definition
        .graph
        .wiring()
        .bindings_on(outputs_node)
        .map(|(port, binding)| (port.port, remapped(binding, &fresh)))
        .collect();
    for (input, binding) in held.wiring().bindings() {
        if let Binding::Wire(source) = binding
            && source.node_id == instance
            && let Some(to) = fed.get(&source.port)
        {
            intents.push(GraphIntent::SetInput {
                input,
                to: Some(to.clone()),
            });
        }
    }
    intents.extend(selection);
    intents
}

/// `binding` with a wire's source under its fresh id.
fn remapped(binding: &Binding, fresh: &HashMap<NodeId, NodeId>) -> Binding {
    match binding {
        Binding::Wire(source) => Binding::Wire(OutputPort::new(
            fresh
                .get(&source.node_id)
                .copied()
                .unwrap_or(source.node_id),
            source.port,
        )),
        Binding::Const(value) => Binding::Const(value.clone()),
    }
}

/// A node outside `inside` that both reads a node of it and feeds one, through any path of
/// wires in `graph`: the node between a wire that leaves the selection and a later one that
/// returns. The least such id, so a refusal names the same node each time.
fn feeds_itself(document: &Document, graph: GraphId, inside: &BTreeSet<NodeId>) -> Option<NodeId> {
    let held = document.graphs.graph(graph);
    let mut consumers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut producers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (input, source) in held.wiring().wires() {
        consumers
            .entry(source.node_id)
            .or_default()
            .push(input.node_id);
        producers
            .entry(input.node_id)
            .or_default()
            .push(source.node_id);
    }
    let reach = |edges: &HashMap<NodeId, Vec<NodeId>>| {
        // A walk over the graph's nodes, each visited once: bounded by their count.
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut open: Vec<NodeId> = inside.iter().copied().collect();
        while let Some(node_id) = open.pop() {
            for &next in edges.get(&node_id).into_iter().flatten() {
                if !inside.contains(&next) && seen.insert(next) {
                    open.push(next);
                }
            }
        }
        seen
    };
    let downstream = reach(&consumers);
    let upstream = reach(&producers);
    downstream.intersection(&upstream).next().copied()
}

#[cfg(test)]
mod tests;
