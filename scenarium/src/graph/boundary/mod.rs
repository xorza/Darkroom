//! Authored subgraph-interface mutation: detach or attach one boundary port
//! together with every binding and pin it severs, so a removal is exactly
//! reversible. The receiver is the *owning* graph — the one holding the
//! local child in `graphs` and its instance nodes — because removing a port
//! rewires both the child interior and the owner's instance bindings.

use serde::{Deserialize, Serialize};

use crate::graph::interface::{GraphId, GraphLink};
use crate::graph::wiring::BindingEntry;
use crate::graph::{Binding, Graph, InputPort, NodeId, NodeKind, OutputPort};
use crate::node::definition::{FuncInput, FuncOutput};

/// A subgraph *input* removed from the interface at `idx`: its spec, the
/// interior edges the boundary output fed, pins on that boundary output,
/// and the owning graph's instance bindings into the slot. Ports above
/// `idx` shift down on detach and back up on attach.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetachedGraphInput {
    pub idx: usize,
    pub spec: FuncInput,
    /// Interior consumers fed by the `GraphInput` boundary output `idx`.
    pub interior: Vec<BindingEntry>,
    /// Pins on the boundary output `idx`.
    pub pins: Vec<OutputPort>,
    /// Owning-graph bindings on instance input `idx`.
    pub parent: Vec<BindingEntry>,
}

/// A subgraph *output* removed from the interface at `idx` — the output-side
/// mirror of [`DetachedGraphInput`]: interior wiring is the binding *on* the
/// `GraphOutput` boundary input `idx`, parent wiring is every consumer of
/// (and pin on) instance output `idx`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetachedGraphOutput {
    pub idx: usize,
    pub spec: FuncOutput,
    /// The binding on the `GraphOutput` boundary input `idx`.
    pub interior: Vec<BindingEntry>,
    /// Pins on instance outputs `idx` in the owning graph.
    pub pins: Vec<OutputPort>,
    /// Owning-graph consumers bound to instance output `idx`.
    pub parent: Vec<BindingEntry>,
}

impl Graph {
    /// What [`Self::detach_graph_input`] of `(graph_id, idx)` would remove —
    /// pure, for undo capture. `None` when the child graph, its definition,
    /// or the slot doesn't exist.
    pub fn snapshot_graph_input(
        &self,
        graph_id: GraphId,
        idx: usize,
    ) -> Option<DetachedGraphInput> {
        let child = self.graphs.get(&graph_id)?;
        let spec = child.definition.as_ref()?.inputs.get(idx)?.clone();
        let mut interior = Vec::new();
        let mut pins = Vec::new();
        if let Some(boundary) = child.boundary_node(NodeKind::GraphInput) {
            let port = OutputPort::new(boundary, idx);
            interior = child.bindings_bound_to(port);
            if child.is_output_pinned(port) {
                pins.push(port);
            }
        }
        let parent = self
            .local_instances(graph_id)
            .into_iter()
            .filter_map(|instance| {
                let port = InputPort::new(instance, idx);
                let binding = self.bindings.get(&port)?.clone();
                Some(BindingEntry { port, binding })
            })
            .collect();
        Some(DetachedGraphInput {
            idx,
            spec,
            interior,
            pins,
            parent,
        })
    }

    /// Remove subgraph input `idx` from local child `graph_id`: drop its
    /// spec, sever the interior edges and pins on the boundary output, drop
    /// every instance binding into the slot, and shift the ports above it
    /// down by one on both sides. Returns the removed state for
    /// [`Self::attach_graph_input`].
    pub fn detach_graph_input(&mut self, graph_id: GraphId, idx: usize) -> DetachedGraphInput {
        let detached = self
            .snapshot_graph_input(graph_id, idx)
            .expect("cannot detach a graph input that does not exist");
        let instances = self.local_instances(graph_id);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.definition.as_mut().unwrap().inputs.remove(idx);
        if let Some(boundary) = child.boundary_node(NodeKind::GraphInput) {
            child.bindings.retain(|_, binding| {
                !matches!(binding, Binding::Bind(src) if *src == OutputPort::new(boundary, idx))
            });
            child.shift_bound_values(boundary, idx);
            child.shift_pins(boundary, idx);
        }
        for instance in instances {
            self.bindings.remove(&InputPort::new(instance, idx));
            self.shift_binding_keys(instance, idx);
        }
        detached
    }

    /// Exact inverse of [`Self::detach_graph_input`]: shift ports back up
    /// and restore the spec, interior edges, pins, and instance bindings.
    /// Panics on a malformed record — one whose wiring doesn't reference the
    /// detached slot, or that overlaps wiring created after detachment.
    pub fn attach_graph_input(&mut self, graph_id: GraphId, detached: DetachedGraphInput) {
        let DetachedGraphInput {
            idx,
            spec,
            interior,
            pins,
            parent,
        } = detached;
        let instances = self.local_instances(graph_id);
        for entry in &parent {
            assert!(
                entry.port.port_idx == idx && instances.contains(&entry.port.node_id),
                "detached instance binding does not sit on the detached input slot"
            );
        }
        for instance in &instances {
            self.unshift_binding_keys(*instance, idx);
        }
        for entry in parent {
            let previous = self.bindings.insert(entry.port, entry.binding);
            assert!(
                previous.is_none(),
                "cannot attach over instance bindings created after detachment"
            );
        }
        let child = self
            .graphs
            .get_mut(&graph_id)
            .expect("cannot attach a graph input to a missing graph");
        let definition = child.definition.as_mut().unwrap();
        assert!(idx <= definition.inputs.len(), "attach index out of range");
        definition.inputs.insert(idx, spec);
        match child.boundary_node(NodeKind::GraphInput) {
            Some(boundary) => {
                let slot = OutputPort::new(boundary, idx);
                for entry in &interior {
                    assert!(
                        matches!(&entry.binding, Binding::Bind(src) if *src == slot),
                        "detached interior edge is not fed by the detached input slot"
                    );
                }
                for pin in &pins {
                    assert!(
                        *pin == slot,
                        "detached pin does not sit on the detached input slot"
                    );
                }
                child.unshift_bound_values(boundary, idx);
                child.unshift_pins(boundary, idx);
            }
            None => assert!(
                interior.is_empty() && pins.is_empty(),
                "detached interior wiring without a boundary node"
            ),
        }
        for entry in interior {
            let previous = child.bindings.insert(entry.port, entry.binding);
            assert!(
                previous.is_none(),
                "cannot attach over interior bindings created after detachment"
            );
        }
        for pin in pins {
            assert!(
                child.pinned_outputs.insert(pin),
                "cannot attach over pins created after detachment"
            );
        }
    }

    /// What [`Self::detach_graph_output`] of `(graph_id, idx)` would remove —
    /// pure, for undo capture.
    pub fn snapshot_graph_output(
        &self,
        graph_id: GraphId,
        idx: usize,
    ) -> Option<DetachedGraphOutput> {
        let child = self.graphs.get(&graph_id)?;
        let spec = child.definition.as_ref()?.outputs.get(idx)?.clone();
        let interior = match child.boundary_node(NodeKind::GraphOutput) {
            Some(boundary) => {
                let port = InputPort::new(boundary, idx);
                child
                    .bindings
                    .get(&port)
                    .map(|binding| BindingEntry {
                        port,
                        binding: binding.clone(),
                    })
                    .into_iter()
                    .collect()
            }
            None => Vec::new(),
        };
        let mut pins = Vec::new();
        let mut parent = Vec::new();
        for instance in self.local_instances(graph_id) {
            let port = OutputPort::new(instance, idx);
            parent.extend(self.bindings_bound_to(port));
            if self.is_output_pinned(port) {
                pins.push(port);
            }
        }
        Some(DetachedGraphOutput {
            idx,
            spec,
            interior,
            pins,
            parent,
        })
    }

    /// Remove subgraph output `idx` from local child `graph_id` — the
    /// output-side mirror of [`Self::detach_graph_input`].
    pub fn detach_graph_output(&mut self, graph_id: GraphId, idx: usize) -> DetachedGraphOutput {
        let detached = self
            .snapshot_graph_output(graph_id, idx)
            .expect("cannot detach a graph output that does not exist");
        let instances = self.local_instances(graph_id);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.definition.as_mut().unwrap().outputs.remove(idx);
        if let Some(boundary) = child.boundary_node(NodeKind::GraphOutput) {
            child.bindings.remove(&InputPort::new(boundary, idx));
            child.shift_binding_keys(boundary, idx);
        }
        for instance in instances {
            self.bindings.retain(|_, binding| {
                !matches!(binding, Binding::Bind(src) if *src == OutputPort::new(instance, idx))
            });
            self.shift_bound_values(instance, idx);
            self.shift_pins(instance, idx);
        }
        detached
    }

    /// Exact inverse of [`Self::detach_graph_output`]. Panics on a malformed
    /// record — one whose wiring doesn't reference the detached slot, or
    /// that overlaps wiring created after detachment.
    pub fn attach_graph_output(&mut self, graph_id: GraphId, detached: DetachedGraphOutput) {
        let DetachedGraphOutput {
            idx,
            spec,
            interior,
            pins,
            parent,
        } = detached;
        let instances = self.local_instances(graph_id);
        for entry in &parent {
            assert!(
                matches!(&entry.binding, Binding::Bind(src)
                    if src.port_idx == idx && instances.contains(&src.node_id)),
                "detached consumer binding does not read the detached output slot"
            );
        }
        for pin in &pins {
            assert!(
                pin.port_idx == idx && instances.contains(&pin.node_id),
                "detached pin does not sit on the detached output slot"
            );
        }
        for instance in &instances {
            self.unshift_bound_values(*instance, idx);
            self.unshift_pins(*instance, idx);
        }
        for entry in parent {
            let previous = self.bindings.insert(entry.port, entry.binding);
            assert!(
                previous.is_none(),
                "cannot attach over instance-consumer bindings created after detachment"
            );
        }
        for pin in pins {
            assert!(
                self.pinned_outputs.insert(pin),
                "cannot attach over pins created after detachment"
            );
        }
        let child = self
            .graphs
            .get_mut(&graph_id)
            .expect("cannot attach a graph output to a missing graph");
        let definition = child.definition.as_mut().unwrap();
        assert!(idx <= definition.outputs.len(), "attach index out of range");
        definition.outputs.insert(idx, spec);
        match child.boundary_node(NodeKind::GraphOutput) {
            Some(boundary) => {
                for entry in &interior {
                    assert!(
                        entry.port == InputPort::new(boundary, idx),
                        "detached interior binding does not sit on the detached output slot"
                    );
                }
                child.unshift_binding_keys(boundary, idx);
            }
            None => assert!(
                interior.is_empty(),
                "detached interior wiring without a boundary node"
            ),
        }
        for entry in interior {
            let previous = child.bindings.insert(entry.port, entry.binding);
            assert!(
                previous.is_none(),
                "cannot attach over interior bindings created after detachment"
            );
        }
    }

    /// Ids of this graph's `Graph(Local(graph_id))` instance nodes.
    fn local_instances(&self, graph_id: GraphId) -> Vec<NodeId> {
        self.iter()
            .filter_map(|node| match node.kind {
                NodeKind::Graph(GraphLink::Local(id)) if id == graph_id => Some(node.id),
                _ => None,
            })
            .collect()
    }

    /// This graph's single boundary node of `kind`, if present.
    pub(crate) fn boundary_node(&self, kind: NodeKind) -> Option<NodeId> {
        self.iter()
            .find(|node| node.kind == kind)
            .map(|node| node.id)
    }

    /// Every binding whose value is `Bind(source)`, as recorded entries.
    fn bindings_bound_to(&self, source: OutputPort) -> Vec<BindingEntry> {
        self.bindings
            .iter()
            .filter(|(_, binding)| matches!(binding, Binding::Bind(src) if *src == source))
            .map(|(port, binding)| BindingEntry {
                port: *port,
                binding: binding.clone(),
            })
            .collect()
    }

    /// Rewrite binding *values* `Bind(node, j > idx)` to `j - 1`.
    fn shift_bound_values(&mut self, node: NodeId, idx: usize) {
        for binding in self.bindings.values_mut() {
            if let Binding::Bind(src) = binding
                && src.node_id == node
                && src.port_idx > idx
            {
                src.port_idx -= 1;
            }
        }
    }

    /// Rewrite binding *values* `Bind(node, j >= idx)` to `j + 1`.
    fn unshift_bound_values(&mut self, node: NodeId, idx: usize) {
        for binding in self.bindings.values_mut() {
            if let Binding::Bind(src) = binding
                && src.node_id == node
                && src.port_idx >= idx
            {
                src.port_idx += 1;
            }
        }
    }

    /// Rekey bindings *on* `(node, j > idx)` to `j - 1`, ascending so each
    /// insert lands in the slot the previous removal just vacated.
    fn shift_binding_keys(&mut self, node: NodeId, idx: usize) {
        let keys: Vec<InputPort> = self
            .bindings
            .range(InputPort::new(node, idx + 1)..=InputPort::new(node, usize::MAX))
            .map(|(port, _)| *port)
            .collect();
        for port in keys {
            let binding = self.bindings.remove(&port).unwrap();
            self.bindings
                .insert(InputPort::new(node, port.port_idx - 1), binding);
        }
    }

    /// Rekey bindings *on* `(node, j >= idx)` to `j + 1`, descending so the
    /// target slot is always free.
    fn unshift_binding_keys(&mut self, node: NodeId, idx: usize) {
        let keys: Vec<InputPort> = self
            .bindings
            .range(InputPort::new(node, idx)..=InputPort::new(node, usize::MAX))
            .map(|(port, _)| *port)
            .collect();
        for port in keys.into_iter().rev() {
            let binding = self.bindings.remove(&port).unwrap();
            self.bindings
                .insert(InputPort::new(node, port.port_idx + 1), binding);
        }
    }

    /// Drop the pin on `(node, idx)` and shift pins `(node, j > idx)` down.
    fn shift_pins(&mut self, node: NodeId, idx: usize) {
        self.pinned_outputs.remove(&OutputPort::new(node, idx));
        let pins: Vec<OutputPort> = self
            .pinned_outputs
            .range(OutputPort::new(node, idx + 1)..=OutputPort::new(node, usize::MAX))
            .copied()
            .collect();
        for pin in pins {
            self.pinned_outputs.remove(&pin);
            self.pinned_outputs
                .insert(OutputPort::new(node, pin.port_idx - 1));
        }
    }

    /// Shift pins `(node, j >= idx)` up — the inverse of [`Self::shift_pins`]'s
    /// shift half (the dropped pin itself is restored by the caller).
    fn unshift_pins(&mut self, node: NodeId, idx: usize) {
        let pins: Vec<OutputPort> = self
            .pinned_outputs
            .range(OutputPort::new(node, idx)..=OutputPort::new(node, usize::MAX))
            .copied()
            .collect();
        for pin in pins.into_iter().rev() {
            self.pinned_outputs.remove(&pin);
            self.pinned_outputs
                .insert(OutputPort::new(node, pin.port_idx + 1));
        }
    }
}

#[cfg(test)]
mod tests;
