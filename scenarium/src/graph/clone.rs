use hashbrown::HashMap;

use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{Binding, Graph, GraphDef, InputPort, NodeId, NodeKind, Subscription};

/// A remapped clone alongside the node mapping that produced it, so a caller
/// holding ids into the original — a definition's exposed-event emitters —
/// can follow them across the copy.
#[derive(Debug)]
pub(crate) struct MappedClone {
    pub graph: Graph,
    pub node_ids: HashMap<NodeId, NodeId>,
}

impl Graph {
    /// Clone with every node *and* nested-graph identity remapped to a fresh
    /// one, throughout the local graph tree. Both id kinds are unique across
    /// a whole document, so every copy boundary must sever both; `Local`
    /// links are rewritten per level (a node references only its own graph's
    /// map — resolution is parent-scoped). `Shared` links name library graphs
    /// and stay as-is.
    pub fn clone_mapped(&self) -> Graph {
        self.clone_mapped_with_ids().graph
    }

    pub(crate) fn clone_mapped_with_ids(&self) -> MappedClone {
        let mut node_ids = HashMap::with_capacity(self.nodes.len());
        let graph_id_map: HashMap<GraphId, GraphId> = self
            .graphs
            .keys()
            .map(|graph_id| (*graph_id, GraphId::unique()))
            .collect();
        let mut nodes = HashMap::with_capacity(self.nodes.len());
        for (node_id, node) in &self.nodes {
            let new_id = NodeId::unique();
            node_ids.insert(*node_id, new_id);
            let mut node = node.clone();
            // A dangling link (def already missing) keeps its old id —
            // drift tolerance, same as everywhere else.
            if let NodeKind::Graph(GraphLink::Local(graph_id)) = &mut node.kind
                && let Some(new_graph_id) = graph_id_map.get(graph_id)
            {
                *graph_id = *new_graph_id;
            }
            nodes.insert(new_id, node);
        }
        let remap = |id: NodeId| node_ids.get(&id).copied().unwrap_or(id);
        let bindings = self
            .bindings
            .iter()
            .map(|(port, binding)| {
                let port = InputPort::new(remap(port.node_id), port.port_idx);
                let binding = match binding {
                    Binding::Bind(output) => Binding::bind(remap(output.node_id), output.port_idx),
                    other => other.clone(),
                };
                (port, binding)
            })
            .collect();
        let subscriptions = self
            .subscriptions
            .iter()
            .map(|subscription| Subscription {
                emitter: remap(subscription.emitter),
                event_idx: subscription.event_idx,
                subscriber: remap(subscription.subscriber),
            })
            .collect();
        let graphs = self
            .graphs
            .iter()
            .map(|(graph_id, def)| (graph_id_map[graph_id], def.clone_mapped()))
            .collect();
        let graph = Graph {
            nodes,
            bindings,
            subscriptions,
            graphs,
        };
        MappedClone { graph, node_ids }
    }

    /// Clone keeping every identity — node ids and nested graph ids —
    /// exactly as they are. The counterpart to [`Self::clone_mapped`], and
    /// the reason `Graph` isn't `Clone`: preserving identities is only sound
    /// where the original is *not* concurrently present in the same
    /// document, i.e. undo/redo replay of a stored step and library
    /// composition. Anywhere else, use `clone_mapped`.
    pub fn clone_verbatim(&self) -> Graph {
        Graph {
            nodes: self.nodes.clone(),
            bindings: self.bindings.clone(),
            subscriptions: self.subscriptions.clone(),
            graphs: self
                .graphs
                .iter()
                .map(|(graph_id, def)| (*graph_id, def.clone_verbatim()))
                .collect(),
        }
    }
}

impl GraphDef {
    /// [`Graph::clone_mapped`] for a definition: remapped identities and no
    /// library lineage. The interface travels along, with exposed-event
    /// emitters following the remapped interior nodes.
    pub fn clone_mapped(&self) -> Self {
        let mapped = self.body.clone_mapped_with_ids();
        let mut interface = self.interface.clone();
        interface.origin = None;
        for event in &mut interface.events {
            event.emitter = mapped
                .node_ids
                .get(&event.emitter)
                .copied()
                .unwrap_or(event.emitter);
        }
        Self {
            interface,
            body: mapped.graph,
        }
    }

    /// [`Graph::clone_verbatim`] for a definition, with the same soundness
    /// condition.
    pub fn clone_verbatim(&self) -> Self {
        Self {
            interface: self.interface.clone(),
            body: self.body.clone_verbatim(),
        }
    }
}
