//! Read-only projections off a compiled [`ExecutionEngine`]: per-node argument
//! values and the live event triggers the worker spawns to drive the event
//! loop. These read the program and cache only; they never schedule or run.

use crate::execution::ExecutionEngine;
use crate::execution::event::{EventRef, EventTrigger};
use crate::execution::stats::ExecutionStats;

impl ExecutionEngine {
    /// Collect every (event → lambda → state) triple that is currently "live" —
    /// node was executed or cached this run, the event has at least one
    /// subscriber, and its lambda is populated. Used by the worker to spawn the
    /// tasks that drive the event loop.
    pub(crate) fn active_event_triggers(&self, stats: &ExecutionStats) -> Vec<EventTrigger> {
        stats
            .cached_nodes
            .iter()
            .copied()
            .chain(stats.executed_nodes.iter().map(|n| n.node_id))
            .flat_map(|node_id| {
                let e_node = &self.compiled.program.e_nodes[&node_id];
                let event_state = self.cache.slots[&node_id].event_state.clone();
                self.compiled.program.events[e_node.events.range()]
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| !event.subscribers.is_empty() && !event.lambda.is_none())
                    .map(move |(event_idx, event)| EventTrigger {
                        event: EventRef { node_id, event_idx },
                        lambda: event.lambda.clone(),
                        state: event_state.clone(),
                    })
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::DynamicValue;
    use crate::execution::ExecutionEngine;
    use crate::execution::compile::CompiledGraph;
    use crate::execution::identity::{ExecutionNodeId, NodeAddress};
    use crate::execution::program::ExecutionBinding;
    use crate::graph::NodeId;

    pub(crate) fn resolve_node_id(
        compiled: &CompiledGraph,
        address: &NodeAddress,
    ) -> Option<ExecutionNodeId> {
        let flat_id = compiled.flatten_map.flat_node(address)?;
        compiled
            .program
            .e_nodes
            .contains_key(&flat_id)
            .then_some(flat_id)
    }

    #[derive(Debug, Default)]
    pub(crate) struct ArgumentValues {
        pub(crate) inputs: Vec<Option<DynamicValue>>,
        pub(crate) outputs: Vec<DynamicValue>,
    }

    impl ExecutionEngine {
        /// Resident-only argument values, test inspection only: reads whatever is
        /// in RAM, so a disk-only (not-yet-hydrated) node reads back empty.
        pub(crate) fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
            let node_id = resolve_node_id(&self.compiled, &NodeAddress::root(*node_id))?;
            Some(self.argument_values_at(node_id))
        }

        pub(crate) fn get_argument_values_at(
            &self,
            address: &NodeAddress,
        ) -> Option<ArgumentValues> {
            let node_id = resolve_node_id(&self.compiled, address)?;
            Some(self.argument_values_at(node_id))
        }

        fn argument_values_at(&self, node_id: ExecutionNodeId) -> ArgumentValues {
            let e_node = &self.compiled.program.e_nodes[&node_id];

            let inputs = self.compiled.program.inputs[e_node.inputs.range()]
                .iter()
                .map(|input| match &input.binding {
                    ExecutionBinding::None => None,
                    ExecutionBinding::Const(v) => Some(DynamicValue::from(v)),
                    ExecutionBinding::Bind(addr) => self.cache.slots[&addr.target]
                        .output_values()
                        .and_then(|o| o.get(addr.port_idx))
                        .cloned(),
                })
                .collect();

            let outputs = self.cache.slots[&node_id]
                .output_values()
                .map(|o| o.to_vec())
                .unwrap_or_default();

            ArgumentValues { inputs, outputs }
        }
    }
}
