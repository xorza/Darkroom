//! How values move through one run: a node's inputs in and the last-read releases that
//! follow. The state they act on is
//! [`ExecutionFrame`](crate::execution::executor::ExecutionFrame), whose loop steps live
//! beside it in the parent module.

use crate::DynamicValue;
use crate::execution::executor::ExecutionFrame;
use crate::execution::program::index::{NodeIdx, OutputAddr, OutputColumn, OutputIdx};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
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
    pub(super) fn collect_inputs(&mut self, node_idx: NodeIdx) {
        self.inputs.clear();
        for input in &self.program.inputs[self.program[node_idx].inputs] {
            let binding = &input.binding;
            let value = match binding {
                ExecutionBinding::None => DynamicValue::Unbound,
                ExecutionBinding::Const(value) => value.into(),
                ExecutionBinding::Bind(addr) if !self.producer_runs(*addr) => {
                    // Nothing will produce this. Only an *optional* input
                    // can reach here — `input_missing` turns a required
                    // one into a `MissingInputs` verdict, so this node
                    // would not be running at all — and unbound is
                    // precisely what optional means. The resolver planned
                    // no read for it either, so none is completed.
                    DynamicValue::Unbound
                }
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

    /// Whether the plan will run `addr`'s producer — the same predicate the
    /// resolver registers reader counts by, so a read is completed exactly
    /// when one was planned.
    fn producer_runs(&self, addr: OutputAddr) -> bool {
        self.plan.verdicts[addr.node_idx].wants_execute()
    }

    /// Abandons every bound-input read owned by a consumer that will not invoke, allowing
    /// non-RAM producer values to be released as soon as their remaining readers disappear.
    pub(super) fn abandon_input_reads(&mut self, consumer_idx: NodeIdx) {
        for input in &self.program.inputs[self.program[consumer_idx].inputs] {
            if let ExecutionBinding::Bind(address) = &input.binding
                && self.producer_runs(*address)
            {
                self.complete_planned_read(*address);
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
