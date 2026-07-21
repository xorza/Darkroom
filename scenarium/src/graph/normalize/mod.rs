//! Keep graph interfaces and wiring consistent with the graph tree and library.

use std::collections::HashMap;

use crate::data::type_system::DataType;
use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeKind, NodeSearch, Subscription};
use crate::library::Library;
use crate::node::definition::{FuncInput, FuncOutput};

#[derive(Debug)]
struct SidePlan<T> {
    boundary: NodeId,
    interface: Vec<T>,
    remap: HashMap<usize, usize>,
    changed: bool,
}

impl Graph {
    /// Restore derived graph interfaces and remove wiring that no longer
    /// resolves against the resulting graph tree and library.
    pub fn normalize(&mut self, library: &Library) {
        self.normalize_interfaces(library);
        self.prune_dangling_wiring(library);
    }

    fn normalize_interfaces(&mut self, library: &Library) {
        let graph_ids: Vec<GraphId> = self.graphs.keys().copied().collect();
        for graph_id in graph_ids {
            self.graphs
                .get_mut(&graph_id)
                .unwrap()
                .normalize_interfaces(library);
            normalize_local_graph(self, graph_id, library);
        }
    }

    fn prune_dangling_wiring(&mut self, library: &Library) {
        let mut bindings = std::mem::take(&mut self.bindings);
        bindings.retain(|destination, binding| self.binding_live(*destination, binding, library));
        self.bindings = bindings;

        let mut subscriptions = std::mem::take(&mut self.subscriptions);
        subscriptions.retain(|subscription| self.subscription_live(subscription, library));
        self.subscriptions = subscriptions;

        for graph in self.graphs.values_mut() {
            graph.prune_dangling_wiring(library);
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

fn normalize_local_graph(parent: &mut Graph, graph_id: GraphId, library: &Library) {
    let (inputs, outputs) = {
        let graph = parent.graphs.get(&graph_id).unwrap();
        (plan_inputs(graph, library), plan_outputs(graph, library))
    };
    let input_changed = inputs.as_ref().is_some_and(|plan| plan.changed);
    let output_changed = outputs.as_ref().is_some_and(|plan| plan.changed);
    if !input_changed && !output_changed {
        return;
    }

    let instances: Vec<NodeId> = parent
        .iter()
        .filter_map(|node| match node.kind {
            NodeKind::Graph(GraphLink::Local(id)) if id == graph_id => Some(node.id),
            _ => None,
        })
        .collect();

    let graph = parent.graphs.get_mut(&graph_id).unwrap();
    if let Some(plan) = &inputs {
        remap_source_edges(graph, plan.boundary, &plan.remap);
        graph.inputs = plan.interface.clone();
    }
    if let Some(plan) = &outputs {
        remap_target_bindings(graph, plan.boundary, &plan.remap);
        graph.outputs = plan.interface.clone();
    }

    for node_id in instances {
        if let Some(plan) = &inputs {
            remap_target_bindings(parent, node_id, &plan.remap);
        }
        if let Some(plan) = &outputs {
            remap_source_edges(parent, node_id, &plan.remap);
        }
    }
}

fn plan_inputs(graph: &Graph, library: &Library) -> Option<SidePlan<FuncInput>> {
    let boundary = graph
        .iter()
        .find(|node| matches!(node.kind, NodeKind::GraphInput))
        .map(|node| node.id)?;
    let used = used_sorted(
        graph
            .edges()
            .filter_map(|(_, source)| (source.node_id == boundary).then_some(source.port_idx)),
    );
    let mut remap = HashMap::with_capacity(used.len());
    let mut interface = Vec::with_capacity(used.len());
    for (new_idx, &old_idx) in used.iter().enumerate() {
        remap.insert(old_idx, new_idx);
        let data_type = infer_used_input_type(graph, library, boundary, old_idx);
        interface.push(match graph.inputs.get(old_idx) {
            Some(existing) => FuncInput {
                data_type,
                ..existing.clone()
            },
            None => synth_input(new_idx, data_type),
        });
    }
    let changed = interface != graph.inputs || remap.iter().any(|(old, new)| old != new);
    Some(SidePlan {
        boundary,
        interface,
        remap,
        changed,
    })
}

fn plan_outputs(graph: &Graph, library: &Library) -> Option<SidePlan<FuncOutput>> {
    let boundary = graph
        .iter()
        .find(|node| matches!(node.kind, NodeKind::GraphOutput))
        .map(|node| node.id)?;
    let used = used_sorted(
        graph
            .bindings_touching(boundary)
            .into_iter()
            .filter_map(|entry| (entry.port.node_id == boundary).then_some(entry.port.port_idx)),
    );
    let mut remap = HashMap::with_capacity(used.len());
    let mut interface = Vec::with_capacity(used.len());
    for (new_idx, &old_idx) in used.iter().enumerate() {
        remap.insert(old_idx, new_idx);
        let data_type = infer_used_output_type(graph, library, boundary, old_idx);
        let name = graph
            .outputs
            .get(old_idx)
            .map_or_else(|| format!("output{new_idx}"), |output| output.name.clone());
        interface.push(FuncOutput::new(name, data_type));
    }
    let changed = interface != graph.outputs || remap.iter().any(|(old, new)| old != new);
    Some(SidePlan {
        boundary,
        interface,
        remap,
        changed,
    })
}

fn remap_source_edges(graph: &mut Graph, node_id: NodeId, remap: &HashMap<usize, usize>) {
    let edges: Vec<(InputPort, usize)> = graph
        .edges()
        .filter(|(_, source)| source.node_id == node_id)
        .map(|(target, source)| (target, source.port_idx))
        .collect();
    for (target, old_idx) in edges {
        match remap.get(&old_idx) {
            Some(&new_idx) if new_idx != old_idx => {
                graph.set_input_binding(target, Binding::bind(node_id, new_idx));
            }
            Some(_) => {}
            None => graph.set_input_binding(target, None),
        }
    }
}

fn remap_target_bindings(graph: &mut Graph, node_id: NodeId, remap: &HashMap<usize, usize>) {
    let bindings: Vec<(usize, Binding)> = graph
        .bindings_touching(node_id)
        .into_iter()
        .filter(|entry| entry.port.node_id == node_id)
        .map(|entry| (entry.port.port_idx, entry.binding))
        .collect();
    if bindings.is_empty() {
        return;
    }
    for (old_idx, _) in &bindings {
        graph.set_input_binding(InputPort::new(node_id, *old_idx), None);
    }
    for (old_idx, binding) in bindings {
        if let Some(&new_idx) = remap.get(&old_idx) {
            graph.set_input_binding(InputPort::new(node_id, new_idx), binding);
        }
    }
}

fn used_sorted(indices: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut indices: Vec<usize> = indices.collect();
    indices.sort_unstable();
    indices.dedup();
    indices
}

fn synth_input(idx: usize, data_type: DataType) -> FuncInput {
    FuncInput::optional(format!("input{idx}"), data_type)
}

fn infer_used_input_type(
    graph: &Graph,
    library: &Library,
    boundary: NodeId,
    old_idx: usize,
) -> DataType {
    graph
        .edges()
        .find(|(_, source)| source.node_id == boundary && source.port_idx == old_idx)
        .and_then(|(target, _)| graph.input_type(library, target))
        .unwrap_or_default()
}

fn infer_used_output_type(
    graph: &Graph,
    library: &Library,
    boundary: NodeId,
    old_idx: usize,
) -> DataType {
    graph
        .bindings_touching(boundary)
        .into_iter()
        .find_map(|entry| match entry.binding {
            Binding::Bind(source)
                if entry.port.node_id == boundary && entry.port.port_idx == old_idx =>
            {
                Some(source)
            }
            _ => None,
        })
        .map(|source| graph.resolve_output_type(library, source))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
