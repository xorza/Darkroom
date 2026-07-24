//! Authored subgraph-interface mutation: detach or attach one boundary port
//! together with every binding and pin it severs, so a removal is exactly
//! reversible. The receiver is the *owning* graph — the one holding the
//! local child in `graphs` and its instance nodes — because removing a port
//! rewires both the child interior and the owner's instance bindings.
//!
//! Both sides run on the same two primitives, because an interface port is one
//! port slot seen from two directions. An interface *input* is an output slot
//! on the child's inbound boundary node and an input slot on every instance;
//! an interface *output* is the exact swap. So `detach_graph_input` and
//! `detach_graph_output` are the same pair of slot removals, applied to
//! opposite sides.

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

impl DetachedGraphInput {
    /// Panic unless every recorded edge sits on this record's slot: instance
    /// bindings on instance input `idx`, interior edges fed by (and pins on)
    /// the boundary output `idx`. A `None` boundary — no inbound boundary node
    /// in the child body — admits only a record with no interior wiring. Runs
    /// before `attach` mutates anything, so a malformed record can't
    /// half-apply.
    fn assert_targets_slot(&self, instances: &[NodeId], boundary: Option<NodeId>) {
        for entry in &self.parent {
            assert!(
                entry.port.port_idx == self.idx && instances.contains(&entry.port.node_id),
                "detached instance binding does not sit on the detached input slot"
            );
        }
        let Some(boundary) = boundary else {
            assert!(
                self.interior.is_empty() && self.pins.is_empty(),
                "detached interior wiring without a boundary node"
            );
            return;
        };
        let slot = OutputPort::new(boundary, self.idx);
        for entry in &self.interior {
            assert!(
                matches!(&entry.binding, Binding::Bind(src) if *src == slot),
                "detached interior edge is not fed by the detached input slot"
            );
        }
        for pin in &self.pins {
            assert!(
                *pin == slot,
                "detached pin does not sit on the detached input slot"
            );
        }
    }
}

impl DetachedGraphOutput {
    /// The output-side mirror of
    /// [`DetachedGraphInput::assert_targets_slot`]: consumers must *read*
    /// instance output `idx` and pins must sit on it, while the lone interior
    /// binding sits *on* the boundary input `idx`.
    fn assert_targets_slot(&self, instances: &[NodeId], boundary: Option<NodeId>) {
        for entry in &self.parent {
            assert!(
                matches!(&entry.binding, Binding::Bind(src)
                    if src.port_idx == self.idx && instances.contains(&src.node_id)),
                "detached consumer binding does not read the detached output slot"
            );
        }
        for pin in &self.pins {
            assert!(
                pin.port_idx == self.idx && instances.contains(&pin.node_id),
                "detached pin does not sit on the detached output slot"
            );
        }
        let Some(boundary) = boundary else {
            assert!(
                self.interior.is_empty(),
                "detached interior wiring without a boundary node"
            );
            return;
        };
        for entry in &self.interior {
            assert!(
                entry.port == InputPort::new(boundary, self.idx),
                "detached interior binding does not sit on the detached output slot"
            );
        }
    }
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
        let spec = child.definition.inputs.get(idx)?.clone();
        let interior = match child.body.boundary_node(NodeKind::GraphInput) {
            Some(boundary) => child.body.output_slot_wiring(boundary, idx),
            None => OutputSlotWiring::default(),
        };
        let parent = self
            .local_instances(graph_id)
            .into_iter()
            .filter_map(|instance| self.input_slot_binding(instance, idx))
            .collect();
        Some(DetachedGraphInput {
            idx,
            spec,
            interior: interior.consumers,
            pins: interior.pins,
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
        child.definition.inputs.remove(idx);
        if let Some(boundary) = child.body.boundary_node(NodeKind::GraphInput) {
            child.body.remove_output_slot(boundary, idx);
        }
        for instance in instances {
            self.remove_input_slot(instance, idx);
        }
        detached
    }

    /// Exact inverse of [`Self::detach_graph_input`]: shift ports back up
    /// and restore the spec, interior edges, pins, and instance bindings.
    /// Panics on a malformed record — one whose wiring doesn't reference the
    /// detached slot, or that overlaps wiring created after detachment.
    pub fn attach_graph_input(&mut self, graph_id: GraphId, detached: DetachedGraphInput) {
        let instances = self.local_instances(graph_id);
        let child = self
            .graphs
            .get(&graph_id)
            .expect("cannot attach a graph input to a missing graph");
        let boundary = child.body.boundary_node(NodeKind::GraphInput);
        assert!(
            detached.idx <= child.definition.inputs.len(),
            "attach index out of range"
        );
        detached.assert_targets_slot(&instances, boundary);

        let DetachedGraphInput {
            idx,
            spec,
            interior,
            pins,
            parent,
        } = detached;
        for instance in &instances {
            self.insert_input_slot(*instance, idx);
        }
        self.restore_bindings(parent);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.definition.inputs.insert(idx, spec);
        if let Some(boundary) = boundary {
            child.body.insert_output_slot(boundary, idx);
        }
        child.body.restore_bindings(interior);
        child.body.restore_pins(pins);
    }

    /// What [`Self::detach_graph_output`] of `(graph_id, idx)` would remove —
    /// pure, for undo capture.
    pub fn snapshot_graph_output(
        &self,
        graph_id: GraphId,
        idx: usize,
    ) -> Option<DetachedGraphOutput> {
        let child = self.graphs.get(&graph_id)?;
        let spec = child.definition.outputs.get(idx)?.clone();
        let interior = match child.body.boundary_node(NodeKind::GraphOutput) {
            Some(boundary) => child
                .body
                .input_slot_binding(boundary, idx)
                .into_iter()
                .collect(),
            None => Vec::new(),
        };
        let mut exterior = OutputSlotWiring::default();
        for instance in self.local_instances(graph_id) {
            let instance_slot = self.output_slot_wiring(instance, idx);
            exterior.consumers.extend(instance_slot.consumers);
            exterior.pins.extend(instance_slot.pins);
        }
        Some(DetachedGraphOutput {
            idx,
            spec,
            interior,
            pins: exterior.pins,
            parent: exterior.consumers,
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
        child.definition.outputs.remove(idx);
        if let Some(boundary) = child.body.boundary_node(NodeKind::GraphOutput) {
            child.body.remove_input_slot(boundary, idx);
        }
        for instance in instances {
            self.remove_output_slot(instance, idx);
        }
        detached
    }

    /// Exact inverse of [`Self::detach_graph_output`]. Panics on a malformed
    /// record — one whose wiring doesn't reference the detached slot, or
    /// that overlaps wiring created after detachment.
    pub fn attach_graph_output(&mut self, graph_id: GraphId, detached: DetachedGraphOutput) {
        let instances = self.local_instances(graph_id);
        let child = self
            .graphs
            .get(&graph_id)
            .expect("cannot attach a graph output to a missing graph");
        let boundary = child.body.boundary_node(NodeKind::GraphOutput);
        assert!(
            detached.idx <= child.definition.outputs.len(),
            "attach index out of range"
        );
        detached.assert_targets_slot(&instances, boundary);

        let DetachedGraphOutput {
            idx,
            spec,
            interior,
            pins,
            parent,
        } = detached;
        for instance in &instances {
            self.insert_output_slot(*instance, idx);
        }
        self.restore_bindings(parent);
        self.restore_pins(pins);
        let child = self.graphs.get_mut(&graph_id).unwrap();
        child.definition.outputs.insert(idx, spec);
        if let Some(boundary) = boundary {
            child.body.insert_input_slot(boundary, idx);
        }
        child.body.restore_bindings(interior);
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

    /// What [`Self::remove_output_slot`] would sever.
    fn output_slot_wiring(&self, node: NodeId, idx: usize) -> OutputSlotWiring {
        let slot = OutputPort::new(node, idx);
        OutputSlotWiring {
            consumers: self
                .bindings
                .iter()
                .filter(|(_, binding)| matches!(binding, Binding::Bind(src) if *src == slot))
                .map(|(port, binding)| BindingEntry {
                    port: *port,
                    binding: binding.clone(),
                })
                .collect(),
            pins: self
                .is_output_pinned(slot)
                .then_some(slot)
                .into_iter()
                .collect(),
        }
    }

    /// What [`Self::remove_input_slot`] would drop.
    fn input_slot_binding(&self, node: NodeId, idx: usize) -> Option<BindingEntry> {
        let port = InputPort::new(node, idx);
        self.bindings.get(&port).map(|binding| BindingEntry {
            port,
            binding: binding.clone(),
        })
    }

    /// Sever output port `(node, idx)` — its consumers and its pin — and
    /// compact the ports above it down onto the freed slot.
    fn remove_output_slot(&mut self, node: NodeId, idx: usize) {
        let slot = OutputPort::new(node, idx);
        self.bindings
            .retain(|_, binding| !matches!(binding, Binding::Bind(src) if *src == slot));
        self.pinned_outputs.remove(&slot);
        self.shift_bound_values(node, idx, Shift::Down);
        self.shift_pins(node, idx, Shift::Down);
    }

    /// Open a free output slot at `(node, idx)`; the caller restores the
    /// wiring it carried.
    fn insert_output_slot(&mut self, node: NodeId, idx: usize) {
        self.shift_bound_values(node, idx, Shift::Up);
        self.shift_pins(node, idx, Shift::Up);
    }

    /// Drop the binding on input port `(node, idx)` and compact the ports
    /// above it down onto the freed slot.
    fn remove_input_slot(&mut self, node: NodeId, idx: usize) {
        self.bindings.remove(&InputPort::new(node, idx));
        self.shift_binding_keys(node, idx, Shift::Down);
    }

    /// Open a free input slot at `(node, idx)`.
    fn insert_input_slot(&mut self, node: NodeId, idx: usize) {
        self.shift_binding_keys(node, idx, Shift::Up);
    }

    /// Put severed bindings back, refusing to overwrite wiring authored after
    /// the detachment.
    fn restore_bindings(&mut self, entries: impl IntoIterator<Item = BindingEntry>) {
        for entry in entries {
            let port = entry.port;
            let previous = self.bindings.insert(port, entry.binding);
            assert!(
                previous.is_none(),
                "cannot attach over the binding on {port:?}, created after detachment"
            );
        }
    }

    /// [`Self::restore_bindings`] for pins.
    fn restore_pins(&mut self, pins: impl IntoIterator<Item = OutputPort>) {
        for pin in pins {
            assert!(
                self.pinned_outputs.insert(pin),
                "cannot attach over the pin on {pin:?}, created after detachment"
            );
        }
    }

    /// Renumber binding *values* `Bind(node, j)` around slot `idx`.
    fn shift_bound_values(&mut self, node: NodeId, idx: usize, shift: Shift) {
        let first_moved = shift.first_moved(idx);
        for binding in self.bindings.values_mut() {
            if let Binding::Bind(src) = binding
                && src.node_id == node
                && src.port_idx >= first_moved
            {
                src.port_idx = shift.apply(src.port_idx);
            }
        }
    }

    /// Rekey bindings *on* `(node, j)` around slot `idx`.
    fn shift_binding_keys(&mut self, node: NodeId, idx: usize, shift: Shift) {
        let mut ports: Vec<InputPort> = self
            .bindings
            .range(InputPort::new(node, shift.first_moved(idx))..=InputPort::new(node, usize::MAX))
            .map(|(port, _)| *port)
            .collect();
        shift.order(&mut ports);
        for port in ports {
            let binding = self.bindings.remove(&port).unwrap();
            self.bindings
                .insert(InputPort::new(node, shift.apply(port.port_idx)), binding);
        }
    }

    /// Renumber pins on `(node, j)` around slot `idx`.
    fn shift_pins(&mut self, node: NodeId, idx: usize, shift: Shift) {
        let mut pins: Vec<OutputPort> = self
            .pinned_outputs
            .range(
                OutputPort::new(node, shift.first_moved(idx))..=OutputPort::new(node, usize::MAX),
            )
            .copied()
            .collect();
        shift.order(&mut pins);
        for pin in pins {
            self.pinned_outputs.remove(&pin);
            self.pinned_outputs
                .insert(OutputPort::new(node, shift.apply(pin.port_idx)));
        }
    }
}

/// The wiring hanging off one output port. Pins are a zero-or-one list so the
/// input and output sides accumulate them the same way.
#[derive(Debug, Default)]
struct OutputSlotWiring {
    consumers: Vec<BindingEntry>,
    pins: Vec<OutputPort>,
}

/// Which way a slot's removal or restoration renumbers the ports around it.
/// The two are exact inverses, so one implementation serves both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shift {
    /// A slot was removed: every port above it moves down one.
    Down,
    /// A slot is being restored: it and every port above it move up one.
    Up,
}

impl Shift {
    /// The lowest port index this shift moves, for a slot at `idx`.
    fn first_moved(self, idx: usize) -> usize {
        match self {
            Shift::Down => idx + 1,
            Shift::Up => idx,
        }
    }

    /// Where the port at `port_idx` lands.
    fn apply(self, port_idx: usize) -> usize {
        match self {
            Shift::Down => port_idx - 1,
            Shift::Up => port_idx + 1,
        }
    }

    /// Reorder ports collected ascending so each destination is free when it's
    /// written: compacting down writes low-to-high, opening up writes
    /// high-to-low.
    fn order<T>(self, ports: &mut [T]) {
        if self == Shift::Up {
            ports.reverse();
        }
    }
}

#[cfg(test)]
mod tests;
