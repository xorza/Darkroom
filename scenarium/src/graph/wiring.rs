use std::collections::HashMap;

use hashbrown::HashSet;
use serde::{Deserialize, Serialize};

use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeSearch, OutputPort, Subscription};

fn binding_touches(port: InputPort, binding: &Binding, node_id: NodeId) -> bool {
    port.node_id == node_id || matches!(binding, Binding::Bind(src) if src.node_id == node_id)
}

fn subscription_touches(subscription: &Subscription, node_id: NodeId) -> bool {
    subscription.emitter == node_id || subscription.subscriber == node_id
}

pub fn closes_data_cycle(
    edges: impl Iterator<Item = (NodeId, NodeId)>,
    producer: NodeId,
    consumer: NodeId,
) -> bool {
    if producer == consumer {
        return true;
    }
    let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (source, destination) in edges {
        adjacency.entry(source).or_default().push(destination);
    }
    let mut stack = vec![consumer];
    let mut seen = HashSet::new();
    seen.insert(consumer);
    while let Some(node) = stack.pop() {
        for &next in adjacency.get(&node).into_iter().flatten() {
            if next == producer {
                return true;
            }
            if seen.insert(next) {
                stack.push(next);
            }
        }
    }
    false
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetachedNode {
    pub node_id: NodeId,
    pub node: Node,
    pub bindings: Vec<BindingEntry>,
    pub subscriptions: Vec<Subscription>,
    pub pinned_outputs: Vec<OutputPort>,
}

impl DetachedNode {
    fn validate(&self) -> Result<(), &'static str> {
        if self.node_id.is_nil() {
            return Err("detached node id must not be nil");
        }
        if self.bindings.iter().any(|entry| {
            matches!(entry.binding, Binding::None)
                || !binding_touches(entry.port, &entry.binding, self.node_id)
        }) {
            return Err("detached bindings must be sparse and touch the detached node");
        }
        if self
            .bindings
            .windows(2)
            .any(|entries| entries[0].port >= entries[1].port)
        {
            return Err("detached bindings must have unique ordered ports");
        }
        if self
            .subscriptions
            .iter()
            .any(|subscription| !subscription_touches(subscription, self.node_id))
        {
            return Err("detached subscriptions must touch the detached node");
        }
        if self
            .subscriptions
            .windows(2)
            .any(|subscriptions| subscriptions[0] >= subscriptions[1])
        {
            return Err("detached subscriptions must be unique and ordered");
        }
        if self
            .pinned_outputs
            .iter()
            .any(|port| port.node_id != self.node_id)
        {
            return Err("detached pins must belong to the detached node");
        }
        if self
            .pinned_outputs
            .windows(2)
            .any(|ports| ports[0] >= ports[1])
        {
            return Err("detached pins must be unique and ordered");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingEntry {
    pub port: InputPort,
    pub binding: Binding,
}

impl Graph {
    pub fn snapshot_node(&self, node_id: NodeId) -> Option<DetachedNode> {
        let node = self.find(&node_id, NodeSearch::TopLevel)?.clone();
        Some(DetachedNode {
            node_id,
            node,
            bindings: self.bindings_touching(node_id),
            subscriptions: self
                .subscriptions
                .iter()
                .copied()
                .filter(|subscription| subscription_touches(subscription, node_id))
                .collect(),
            pinned_outputs: self
                .pinned_outputs
                .iter()
                .copied()
                .filter(|port| port.node_id == node_id)
                .collect(),
        })
    }

    pub fn detach_node(&mut self, node_id: NodeId) -> DetachedNode {
        assert!(!node_id.is_nil());
        let detached = self
            .snapshot_node(node_id)
            .expect("cannot detach a node that is not in the graph");
        self.nodes.remove(&node_id);
        self.bindings
            .retain(|port, binding| !binding_touches(*port, binding, node_id));
        self.subscriptions
            .retain(|subscription| !subscription_touches(subscription, node_id));
        self.pinned_outputs.retain(|port| port.node_id != node_id);
        detached
    }

    pub fn attach_node(&mut self, detached: DetachedNode) {
        detached
            .validate()
            .expect("cannot attach an invalid detached node");
        assert!(
            !self.nodes.contains_key(&detached.node_id),
            "cannot attach a node that is already in the graph"
        );
        assert!(
            detached
                .bindings
                .iter()
                .all(|entry| !self.bindings.contains_key(&entry.port)),
            "cannot attach over bindings created after detachment"
        );
        assert!(
            detached
                .subscriptions
                .iter()
                .all(|subscription| !self.subscriptions.contains(subscription)),
            "cannot attach over subscriptions created after detachment"
        );
        assert!(
            detached
                .pinned_outputs
                .iter()
                .all(|port| !self.pinned_outputs.contains(port)),
            "cannot attach over pins created after detachment"
        );

        let DetachedNode {
            node_id,
            node,
            bindings,
            subscriptions,
            pinned_outputs,
        } = detached;
        self.insert(node_id, node);
        self.bindings.extend(
            bindings
                .into_iter()
                .map(|entry| (entry.port, entry.binding)),
        );
        self.subscriptions.extend(subscriptions);
        self.pinned_outputs.extend(pinned_outputs);
    }

    pub fn input_binding(&self, port: InputPort) -> Binding {
        self.bindings.get(&port).cloned().unwrap_or(Binding::None)
    }

    pub fn set_input_binding(&mut self, port: InputPort, binding: Binding) {
        if matches!(binding, Binding::None) {
            self.bindings.remove(&port);
        } else {
            self.bindings.insert(port, binding);
        }
    }

    pub fn node_bindings(
        &self,
        node_id: NodeId,
        arity: usize,
    ) -> impl Iterator<Item = BindingEntry> + '_ {
        (0..arity).map(move |port_idx| BindingEntry {
            port: InputPort::new(node_id, port_idx),
            binding: self.input_binding(InputPort::new(node_id, port_idx)),
        })
    }

    pub fn subscribe(&mut self, emitter: NodeId, event_idx: usize, subscriber: NodeId) {
        self.subscriptions.insert(Subscription {
            emitter,
            event_idx,
            subscriber,
        });
    }

    pub fn unsubscribe(&mut self, emitter: NodeId, event_idx: usize, subscriber: NodeId) {
        self.subscriptions.remove(&Subscription {
            emitter,
            event_idx,
            subscriber,
        });
    }

    pub fn is_subscribed(&self, emitter: NodeId, event_idx: usize, subscriber: NodeId) -> bool {
        self.subscriptions.contains(&Subscription {
            emitter,
            event_idx,
            subscriber,
        })
    }

    pub fn bindings_touching(&self, node_id: NodeId) -> Vec<BindingEntry> {
        self.bindings
            .iter()
            .filter(|(port, binding)| binding_touches(**port, binding, node_id))
            .map(|(port, binding)| BindingEntry {
                port: *port,
                binding: binding.clone(),
            })
            .collect()
    }

    pub fn subscriptions(&self) -> impl Iterator<Item = Subscription> + '_ {
        self.subscriptions.iter().copied()
    }

    pub fn set_output_pinned(&mut self, port: OutputPort, pinned: bool) {
        if pinned {
            self.pinned_outputs.insert(port);
        } else {
            self.pinned_outputs.remove(&port);
        }
    }

    pub fn is_output_pinned(&self, port: OutputPort) -> bool {
        self.pinned_outputs.contains(&port)
    }

    pub fn pinned_outputs(&self) -> impl Iterator<Item = OutputPort> + '_ {
        self.pinned_outputs.iter().copied()
    }

    pub fn subscribers(
        &self,
        emitter: NodeId,
        event_idx: usize,
    ) -> impl Iterator<Item = NodeId> + '_ {
        let lower = Subscription {
            emitter,
            event_idx,
            subscriber: NodeId::nil(),
        };
        let upper = Subscription {
            emitter,
            event_idx: event_idx + 1,
            subscriber: NodeId::nil(),
        };
        self.subscriptions
            .range(lower..upper)
            .map(|subscription| subscription.subscriber)
    }
}
