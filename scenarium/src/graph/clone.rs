use std::collections::HashMap;

use hashbrown::HashMap as NodeMap;

use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{Binding, Graph, InputPort, NodeId, NodeKind, OutputPort, Subscription};

impl Graph {
    /// Copy this graph with fresh node *and* nested-graph identities
    /// throughout its local graph tree. The returned value has no library
    /// lineage. Both id kinds are unique across a whole document, so every
    /// copy boundary must sever both; `Local` links are rewritten per level
    /// (a node references only its own graph's map — resolution is
    /// parent-scoped). `Shared` links name library graphs and stay as-is.
    pub fn fresh_copy(&self) -> Graph {
        let mut id_map = HashMap::with_capacity(self.nodes.len());
        let graph_id_map: HashMap<GraphId, GraphId> = self
            .graphs
            .keys()
            .map(|graph_id| (*graph_id, GraphId::unique()))
            .collect();
        let mut nodes = NodeMap::with_capacity(self.nodes.len());
        for (node_id, node) in &self.nodes {
            let new_id = NodeId::unique();
            id_map.insert(*node_id, new_id);
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
        let remap = |id: NodeId| id_map.get(&id).copied().unwrap_or(id);
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
        let pinned_outputs = self
            .pinned_outputs
            .iter()
            .map(|port| OutputPort::new(remap(port.node_id), port.port_idx))
            .collect();
        let mut definition = self.definition.clone();
        if let Some(definition) = &mut definition {
            definition.origin = None;
            for event in &mut definition.events {
                event.emitter = remap(event.emitter);
            }
        }
        let graphs = self
            .graphs
            .iter()
            .map(|(graph_id, graph)| (graph_id_map[graph_id], graph.fresh_copy()))
            .collect();
        Graph {
            definition,
            nodes,
            bindings,
            subscriptions,
            pinned_outputs,
            graphs,
        }
    }
}
