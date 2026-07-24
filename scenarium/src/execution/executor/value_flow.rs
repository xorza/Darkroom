use tokio::sync::mpsc::UnboundedSender;

use crate::DynamicValue;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::executor::EVENTS_OUTLIVE_RUN;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::index::{OutputColumn, OutputIdx};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::execution::report::{PinnedOutput, PinnedOutputs, RunEvent};
use crate::execution::resolve::ResolvedRun;
use crate::execution::resource::RunResourceStamps;
use crate::node::lambda::InvokeInput;

#[derive(Default, Debug)]
pub(crate) struct RemainingOutputReads {
    pub(crate) counts: OutputColumn<u32>,
}

impl RemainingOutputReads {
    pub(crate) fn seed(&mut self, resolved: &ResolvedRun) {
        self.counts.clone_from(&resolved.outputs.readers);
    }

    fn is_last(&self, output_idx: OutputIdx) -> bool {
        self.counts[output_idx] == 1
    }

    pub(crate) fn consume(&mut self, output_idx: OutputIdx) -> bool {
        let remaining = &mut self.counts[output_idx];
        debug_assert!(
            *remaining > 0,
            "read an output more often than the resolved run counted"
        );
        *remaining = remaining.wrapping_sub(1);
        *remaining == 0
    }

    fn node_drained(&self, program: &ExecutionProgram, e_node_id: ExecutionNodeId) -> bool {
        self.counts
            .slice(program.e_nodes[&e_node_id].outputs)
            .iter()
            .all(|remaining| *remaining == 0)
    }
}

#[derive(Debug)]
pub(crate) struct ExecutionFrame<'a> {
    pub(crate) program: &'a ExecutionProgram,
    pub(crate) plan: &'a ExecutionPlan,
    pub(crate) cache: &'a mut RuntimeCache,
    pub(crate) resource_stamps: &'a mut RunResourceStamps,
    pub(crate) remaining_reads: &'a mut RemainingOutputReads,
    pub(crate) inputs: &'a mut Vec<InvokeInput>,
}

impl ExecutionFrame<'_> {
    pub(crate) fn emit_pinned_values(
        &mut self,
        e_node_id: ExecutionNodeId,
        events: Option<&UnboundedSender<RunEvent>>,
    ) {
        let Some(events) = events else { return };
        let outputs = self.program.e_nodes[&e_node_id].outputs;
        let pinned_root = self.plan.pinned.contains(&e_node_id);
        let values: Vec<_> = self.program.outputs[outputs]
            .iter()
            .enumerate()
            .filter(|(_, output)| pinned_root || output.pinned)
            .map(|(port_idx, _)| {
                let value = self
                    .cache
                    .read_output_port(self.program, e_node_id, port_idx, false)
                    .expect("a node's pinned output must be resident when delivered");
                PinnedOutput { port_idx, value }
            })
            .collect();
        if values.is_empty() {
            return;
        }
        events
            .send(RunEvent::PinnedOutputs(PinnedOutputs { e_node_id, values }))
            .expect(EVENTS_OUTLIVE_RUN);
    }

    pub(crate) fn collect_inputs(&mut self, e_node_id: ExecutionNodeId) {
        self.inputs.clear();
        for input in &self.program.inputs[self.program.e_nodes[&e_node_id].inputs] {
            let binding = &input.binding;
            let value = match binding {
                ExecutionBinding::None => DynamicValue::Unbound,
                ExecutionBinding::Const(value) => value.into(),
                ExecutionBinding::Bind(addr) => {
                    let target = addr.e_node_id;
                    let port_idx = addr.port_idx;
                    let output_idx = self.program.output_idx(target, port_idx);
                    let take = self.remaining_reads.is_last(output_idx)
                        && !self.program.e_nodes[&target].cache.caches_in_ram();
                    let value = self
                        .cache
                        .read_output_port(self.program, target, port_idx, take)
                        .expect("a resolved producer output must be resident when consumed");
                    self.complete_planned_read(target, port_idx, output_idx);
                    value
                }
            };
            self.inputs.push(InvokeInput { value });
        }
    }

    /// Abandons every bound-input read owned by a consumer that will not invoke, allowing
    /// non-RAM producer values to be released as soon as their remaining readers disappear.
    pub(crate) fn abandon_input_reads(&mut self, consumer_id: ExecutionNodeId) {
        for input in &self.program.inputs[self.program.e_nodes[&consumer_id].inputs] {
            let address = match &input.binding {
                ExecutionBinding::Bind(address) => Some(*address),
                ExecutionBinding::None | ExecutionBinding::Const(_) => None,
            };
            if let Some(address) = address {
                let output_idx = self.program.output_idx(address.e_node_id, address.port_idx);
                self.complete_planned_read(address.e_node_id, address.port_idx, output_idx);
            }
        }
    }

    pub(crate) fn release_drained_outputs(&mut self, e_node_id: ExecutionNodeId) {
        if !self.program.e_nodes[&e_node_id].cache.caches_in_ram()
            && self.remaining_reads.node_drained(self.program, e_node_id)
        {
            self.cache.slots.get_mut(&e_node_id).unwrap().clear_output();
        }
    }

    /// Completes one resolver-counted read and releases its producer port or slot when no
    /// planned reader can still use it.
    fn complete_planned_read(
        &mut self,
        producer_id: ExecutionNodeId,
        producer_port_idx: usize,
        output_idx: OutputIdx,
    ) {
        if !self.remaining_reads.consume(output_idx)
            || self.cache.slots[&producer_id].output_values().is_none()
        {
            return;
        }
        if self.remaining_reads.node_drained(self.program, producer_id) {
            self.release_drained_outputs(producer_id);
        } else if !self.program.e_nodes[&producer_id].cache.caches_in_ram() {
            self.cache.clear_output_port(producer_id, producer_port_idx);
        }
    }
}
