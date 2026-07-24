use std::collections::HashMap;

use hashbrown::HashMap as NodeMap;

use crate::graph::{Binding, Graph, InputPort, NodeId, OutputPort, Subscription};

impl Graph {
    /// Copy this graph with fresh node identities throughout its local graph
    /// tree. The returned value has no library lineage.
    pub fn fresh_copy(&self) -> Graph {
        let mut id_map = HashMap::with_capacity(self.nodes.len());
        let mut nodes = NodeMap::with_capacity(self.nodes.len());
        for (node_id, node) in &self.nodes {
            let new_id = NodeId::unique();
            id_map.insert(*node_id, new_id);
            nodes.insert(new_id, node.clone());
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
            .map(|(graph_id, graph)| (*graph_id, graph.fresh_copy()))
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
