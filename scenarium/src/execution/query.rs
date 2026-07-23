//! Read-only projections off a compiled [`ExecutionEngine`]: per-node argument
//! values and the live event triggers the worker spawns to drive the event
//! loop. These read the program and cache only; they never schedule or run.

use crate::execution::ExecutionEngine;
use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::outcome::ExecutionOutcome;

impl ExecutionEngine {
    /// Collect every (event → lambda → state) triple that is currently "live" —
    /// node was executed or cached this run, the event has at least one
    /// subscriber, and its lambda is populated. Used by the worker to spawn the
    /// tasks that drive the event loop.
    pub(crate) fn active_event_triggers(&self, stats: &ExecutionOutcome) -> Vec<EventTrigger> {
        stats
            .cached_nodes
            .iter()
            .copied()
            .chain(stats.executed_nodes.iter().map(|n| n.e_node_id))
            .flat_map(|e_node_id| {
                let e_node = &self.compiled.program.e_nodes[&e_node_id];
                let event_state = self.cache.slots[&e_node_id].event_state.clone();
                self.compiled.program.events[e_node.events.range()]
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| !event.subscribers.is_empty() && !event.lambda.is_none())
                    .map(move |(event_idx, event)| EventTrigger {
                        event: ExecutionEventPort {
                            e_node_id,
                            event_idx,
                        },
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
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::ExecutionBinding;
    use crate::graph::NodeId;

    pub(crate) fn resolve_e_node_id(
        compiled: &CompiledGraph,
        path: &[NodeId],
    ) -> Option<ExecutionNodeId> {
        let e_node_id = ExecutionNodeId::from_authoring(path);
        compiled
            .program
            .e_nodes
            .contains_key(&e_node_id)
            .then_some(e_node_id)
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
            let e_node_id = resolve_e_node_id(&self.compiled, &[*node_id])?;
            Some(self.argument_values_at(e_node_id))
        }

        pub(crate) fn get_argument_values_at(
            &self,
            e_node_id: ExecutionNodeId,
        ) -> Option<ArgumentValues> {
            self.compiled.program.e_nodes.get(&e_node_id)?;
            Some(self.argument_values_at(e_node_id))
        }

        fn argument_values_at(&self, e_node_id: ExecutionNodeId) -> ArgumentValues {
            let e_node = &self.compiled.program.e_nodes[&e_node_id];

            let inputs = self.compiled.program.inputs[e_node.inputs.range()]
                .iter()
                .map(|input| match &input.binding {
                    ExecutionBinding::None => None,
                    ExecutionBinding::Const(v) => Some(DynamicValue::from(v)),
                    ExecutionBinding::Bind(addr) => self.cache.slots[&addr.e_node_id]
                        .output_values()
                        .and_then(|o| o.get(addr.port_idx))
                        .cloned(),
                })
                .collect();

            let outputs = self.cache.slots[&e_node_id]
                .output_values()
                .map(|o| o.to_vec())
                .unwrap_or_default();

            ArgumentValues { inputs, outputs }
        }
    }
}
