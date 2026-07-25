//! How values move through one run: a node's inputs in, its pinned outputs out to the host,
//! and the last-read releases in between. The state they act on is
//! [`ExecutionFrame`](crate::execution::executor::ExecutionFrame), whose loop steps live
//! beside it in the parent module.

use crate::DynamicValue;
use crate::execution::executor::ExecutionFrame;
use crate::execution::program::index::{NodeIdx, OutputAddr, OutputColumn, OutputIdx};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::execution::report::{PinnedOutput, PinnedOutputs};
use crate::execution::resolve::ResolvedRun;

#[derive(Default, Debug)]
pub(super) struct RemainingOutputReads {
    pub(super) counts: OutputColumn<u32>,
}

impl RemainingOutputReads {
    pub(super) fn seed(&mut self, resolved: &ResolvedRun) {
        self.counts.clone_from(&resolved.outputs.readers);
    }

    fn is_last(&self, output_idx: OutputIdx) -> bool {
        self.counts[output_idx] == 1
    }

    pub(super) fn consume(&mut self, output_idx: OutputIdx) -> bool {
        let remaining = &mut self.counts[output_idx];
        debug_assert!(
            *remaining > 0,
            "read an output more often than the resolved run counted"
        );
        *remaining = remaining.wrapping_sub(1);
        *remaining == 0
    }

    fn node_drained(&self, program: &ExecutionProgram, node_idx: NodeIdx) -> bool {
        self.counts
            .slice(program[node_idx].outputs)
            .iter()
            .all(|remaining| *remaining == 0)
    }
}

impl ExecutionFrame<'_, '_> {
    pub(super) fn emit_pinned_values(&mut self, node_idx: NodeIdx) {
        let outputs = self.program[node_idx].outputs;
        let pinned_root = self.plan.pinned.contains(node_idx);
        let values: Vec<_> = self.program.outputs[outputs]
            .iter()
            .enumerate()
            .filter(|(_, output)| pinned_root || output.pinned)
            .map(|(port_idx, _)| {
                let address = OutputAddr {
                    node_idx,
                    port_idx: port_idx as u32,
                };
                let value = self
                    .cache
                    .read_output_port(self.program, address, false)
                    .expect("a node's pinned output must be resident when delivered");
                PinnedOutput { port_idx, value }
            })
            .collect();
        if values.is_empty() {
            return;
        }
        let e_node_id = self.program.e_node_ids[node_idx];
        self.reporter.pinned(PinnedOutputs { e_node_id, values });
    }

    pub(super) fn collect_inputs(&mut self, node_idx: NodeIdx) {
        self.inputs.clear();
        for input in &self.program.inputs[self.program[node_idx].inputs] {
            let binding = &input.binding;
            let value = match binding {
                ExecutionBinding::None => DynamicValue::Unbound,
                ExecutionBinding::Const(value) => value.into(),
                ExecutionBinding::Bind(addr) => {
                    let address = *addr;
                    let output_idx = self.program.output_idx(address);
                    let take = self.remaining_reads.is_last(output_idx)
                        && !self.program[address.node_idx].cache.caches_in_ram();
                    let value = self
                        .cache
                        .read_output_port(self.program, address, take)
                        .expect("a resolved producer output must be resident when consumed");
                    self.complete_planned_read(address);
                    value
                }
            };
            self.inputs.push(value);
        }
    }

    /// Abandons every bound-input read owned by a consumer that will not invoke, allowing
    /// non-RAM producer values to be released as soon as their remaining readers disappear.
    pub(super) fn abandon_input_reads(&mut self, consumer_idx: NodeIdx) {
        for input in &self.program.inputs[self.program[consumer_idx].inputs] {
            let address = match &input.binding {
                ExecutionBinding::Bind(address) => Some(*address),
                ExecutionBinding::None | ExecutionBinding::Const(_) => None,
            };
            if let Some(address) = address {
                self.complete_planned_read(address);
            }
        }
    }

    pub(super) fn release_drained_outputs(&mut self, node_idx: NodeIdx) {
        if !self.program[node_idx].cache.caches_in_ram()
            && self.remaining_reads.node_drained(self.program, node_idx)
        {
            self.cache.slots[node_idx].clear_output();
        }
    }

    /// Completes one resolver-counted read and releases its producer port or slot when no
    /// planned reader can still use it.
    fn complete_planned_read(&mut self, address: OutputAddr) {
        let output_idx = self.program.output_idx(address);
        if !self.remaining_reads.consume(output_idx)
            || self.cache.slots[address.node_idx].output_values().is_none()
        {
            return;
        }
        if self
            .remaining_reads
            .node_drained(self.program, address.node_idx)
        {
            self.release_drained_outputs(address.node_idx);
        } else if !self.program[address.node_idx].cache.caches_in_ram() {
            self.cache.clear_output_port(address);
        }
    }
}
