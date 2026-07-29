//! Event-edge routing across composite boundaries.

use super::Run;
use crate::execution::compile::flat::PendingSubscription;
use crate::execution::identity::ExecutionEventPort;
use crate::graph::identity::NodeId;
use crate::graph::node::NodeKind;
use crate::graph::{Graph, NodeSearch};

impl<'a> Run<'a> {
    /// Resolve this level's event subscriptions across composite boundaries
    /// into flat `(emitter, event_idx, subscriber)` edges. Subscriptions
    /// emitted *by* a `GraphInput` (the trigger) are consumed when the
    /// enclosing instance is resolved as a subscriber, so they are skipped here.
    pub(super) fn collect_subscriptions(&mut self, graph: &'a Graph) {
        let trigger = graph.boundary_node(NodeKind::GraphInput);

        for sub in graph.subscriptions() {
            if Some(sub.emitter) == trigger {
                continue;
            }
            let Some(event) = self.resolve_emitter(sub.emitter, sub.event_idx) else {
                continue;
            };
            self.resolve_subscriber(sub.subscriber, event);
        }
    }

    /// Resolve an emitter `(node, event_idx)` to the concrete flat func event
    /// it ultimately fires, following composite exposed-event mappings inward.
    fn resolve_emitter(&mut self, node_id: NodeId, event_idx: usize) -> Option<ExecutionEventPort> {
        let graph = self.current();
        // Both callers name a validated node: a subscription's emitter, or
        // an interface event's. `None` below is drift or a deliberate skip,
        // never an id this graph has no node for.
        let node = graph
            .find(node_id, NodeSearch::TopLevel)
            .expect("subscription emitter resolved by validate_for_execution");
        if node.disabled {
            return None; // a disabled node fires no events
        }
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => {
                // Drift tolerance: a subscription to an event the func no
                // longer declares wires nothing.
                if graph
                    .node_ports(node, self.library)
                    .is_some_and(|ports| event_idx >= ports.events.len())
                {
                    return None;
                }
                Some(ExecutionEventPort {
                    e_node_id: self.execution_node_id(node_id),
                    event_idx,
                })
            }
            NodeKind::Graph(r) => {
                let nested = graph
                    .resolve_graph(*r, self.library)
                    .expect("graph node references a missing graph");
                // Drift: the interface may no longer expose this event.
                let exposed = nested.events.get(event_idx)?;
                self.push_level(node_id, &nested.body);
                let resolved = self.resolve_emitter(exposed.emitter, exposed.emitter_event_idx);
                self.pop_level();
                resolved
            }
            NodeKind::GraphInput | NodeKind::GraphOutput => None,
        }
    }

    /// Resolve a subscriber to the concrete flat func nodes that actually run,
    /// pushing `(emitter, event_idx, flat_subscriber)` for each. A composite
    /// subscriber expands to the interior nodes wired to its `GraphInput`
    /// trigger.
    fn resolve_subscriber(&mut self, node_id: NodeId, event: ExecutionEventPort) {
        let graph = self.current();
        // Every caller names a validated subscriber, at this level or a
        // nested one.
        let node = graph
            .find(node_id, NodeSearch::TopLevel)
            .expect("subscriber resolved by validate_for_execution");
        // A disabled node runs nothing, so it receives no events.
        if node.disabled {
            return;
        }
        match &node.kind {
            // A special node subscribes like a func: it flattens to one leaf and
            // becomes the flat subscriber. `RunSinks` in particular relies on
            // this edge so the planner sees it among a fired event's subscribers.
            NodeKind::Func(_) | NodeKind::Special(_) => {
                let e_node_id = self.execution_node_id(node_id);
                self.flat.subscriptions.push(PendingSubscription {
                    event,
                    subscriber: e_node_id,
                });
            }
            NodeKind::Graph(r) => {
                let nested = graph
                    .resolve_graph(*r, self.library)
                    .expect("graph node references a missing graph");
                // A definition with no inbound boundary has nothing to
                // deliver the event to — authored, not broken.
                let Some(trigger) = nested.body.boundary_node(NodeKind::GraphInput) else {
                    return;
                };
                self.push_level(node_id, &nested.body);
                for sub in nested.body.subscriptions().filter(|s| s.emitter == trigger) {
                    self.resolve_subscriber(sub.subscriber, event);
                }
                self.pop_level();
            }
            NodeKind::GraphInput | NodeKind::GraphOutput => {}
        }
    }
}
