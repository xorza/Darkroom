use std::collections::HashMap;

use common::KeyIndexVec;
use hashbrown::HashMap as NodeMap;

use crate::graph::{Binding, Graph, InputPort, NodeId, OutputPort, Subscription};

#[derive(Debug)]
pub(crate) struct FreshGraph {
    pub(crate) graph: Graph,
    pub(crate) id_map: HashMap<NodeId, NodeId>,
}

impl Graph {
    pub(crate) fn with_fresh_node_ids(&self) -> FreshGraph {
        let mut id_map = HashMap::with_capacity(self.nodes.len());
        let mut nodes = NodeMap::with_capacity(self.nodes.len());
        for node in self.nodes.values() {
            let new_id = NodeId::unique();
            id_map.insert(node.id, new_id);
            let mut clone = node.clone();
            clone.id = new_id;
            nodes.insert(new_id, clone);
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
        let mut subgraphs = KeyIndexVec::with_capacity(self.subgraphs.len());
        for definition in self.subgraphs.iter() {
            subgraphs.add(definition.remapped_interior());
        }
        let graph = Graph {
            nodes,
            bindings,
            subscriptions,
            pinned_outputs,
            subgraphs,
        };
        FreshGraph { graph, id_map }
    }
}
