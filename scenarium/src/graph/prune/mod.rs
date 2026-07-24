//! Remove wiring that no longer resolves against the graph tree and
//! library — stale references left by external drift (a func that lost
//! ports, a reshaped shared-graph interface, a hand-edited file), applied
//! when a document is loaded rather than per edit.

use crate::graph::{Binding, Graph, InputPort, Node, NodeSearch, Subscription};
use crate::library::Library;

impl Graph {
    /// Recursively drop bindings and subscriptions whose endpoints no
    /// longer resolve against this graph tree and `library`. Idempotent;
    /// references into nodes the library can't resolve at all are kept
    /// (the func may come back).
    pub fn prune(&mut self, library: &Library) {
        let mut bindings = std::mem::take(&mut self.bindings);
        bindings.retain(|destination, binding| self.binding_live(*destination, binding, library));
        self.bindings = bindings;

        let mut subscriptions = std::mem::take(&mut self.subscriptions);
        subscriptions.retain(|subscription| self.subscription_live(subscription, library));
        self.subscriptions = subscriptions;

        for graph in self.graphs.values_mut() {
            graph.prune(library);
        }
    }

    fn binding_live(&self, destination: InputPort, binding: &Binding, library: &Library) -> bool {
        self.find(&destination.node_id, NodeSearch::TopLevel)
            .is_some_and(|consumer| {
                self.port_in_range(consumer, destination.port_idx, true, library)
            })
            && match binding {
                Binding::Bind(source) => self
                    .find(&source.node_id, NodeSearch::TopLevel)
                    .is_some_and(|producer| {
                        self.port_in_range(producer, source.port_idx, false, library)
                    }),
                Binding::Const(_) => true,
            }
    }

    fn subscription_live(&self, subscription: &Subscription, library: &Library) -> bool {
        match self.find(&subscription.emitter, NodeSearch::TopLevel) {
            None => false,
            Some(emitter) => self
                .event_count(emitter, library)
                .is_none_or(|count| subscription.event_idx < count),
        }
    }

    fn port_in_range(&self, node: &Node, idx: usize, input: bool, library: &Library) -> bool {
        let count = if input {
            self.input_count(node, library)
        } else {
            self.output_count(node, library)
        };
        count.is_none_or(|count| idx < count)
    }
}

#[cfg(test)]
mod tests;
