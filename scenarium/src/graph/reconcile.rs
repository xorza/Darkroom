use crate::graph::{Binding, Graph, InputPort, Node, NodeKind, NodeSearch, Subscription};
use crate::library::Library;

impl Graph {
    pub fn prune_dangling_wiring(&mut self, library: &Library) -> usize {
        let mut bindings = std::mem::take(&mut self.bindings);
        let before = bindings.len();
        bindings.retain(|destination, binding| self.binding_live(*destination, binding, library));
        self.bindings = bindings;
        let mut removed = before - self.bindings.len();

        let mut subscriptions = std::mem::take(&mut self.subscriptions);
        let before = subscriptions.len();
        subscriptions.retain(|subscription| self.subscription_live(subscription, library));
        self.subscriptions = subscriptions;
        removed += before - self.subscriptions.len();

        for definition in self.subgraphs.iter_mut() {
            removed += definition.graph.prune_dangling_wiring(library);
        }
        removed
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
                Binding::None | Binding::Const(_) => true,
            }
    }

    fn subscription_live(&self, subscription: &Subscription, library: &Library) -> bool {
        match self.find(&subscription.emitter, NodeSearch::TopLevel) {
            None => false,
            Some(emitter) => self
                .event_count_opt(emitter, library)
                .is_none_or(|count| subscription.event_idx < count),
        }
    }

    fn port_in_range(&self, node: &Node, idx: usize, input: bool, library: &Library) -> bool {
        let in_range = |inputs: usize, outputs: usize| idx < if input { inputs } else { outputs };
        match &node.kind {
            NodeKind::Func(id) => library
                .by_id(id)
                .is_none_or(|function| in_range(function.inputs.len(), function.outputs.len())),
            NodeKind::Subgraph(reference) => {
                self.resolve_def(*reference, library)
                    .is_none_or(|definition| {
                        in_range(definition.inputs.len(), definition.outputs.len())
                    })
            }
            NodeKind::Special(special) => {
                let function = special.func();
                in_range(function.inputs.len(), function.outputs.len())
            }
            NodeKind::SubgraphInput | NodeKind::SubgraphOutput => true,
        }
    }

    pub(crate) fn event_count_opt(&self, node: &Node, library: &Library) -> Option<usize> {
        match &node.kind {
            NodeKind::Func(id) => library.by_id(id).map(|function| function.events.len()),
            NodeKind::Subgraph(reference) => self
                .resolve_def(*reference, library)
                .map(|definition| definition.events.len()),
            NodeKind::Special(special) => Some(special.func().events.len()),
            NodeKind::SubgraphInput => Some(1),
            NodeKind::SubgraphOutput => Some(0),
        }
    }
}
