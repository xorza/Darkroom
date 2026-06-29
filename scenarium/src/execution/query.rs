//! Read-only projections off a compiled [`ExecutionEngine`]: per-node argument
//! values (optionally with awaited previews) and the live event triggers the
//! worker spawns to drive the event loop. These read the program and cache only;
//! they never schedule or run.

use crate::data::DynamicValue;
use crate::execution::event::{EventRef, EventTrigger};
use crate::execution::program::{ExecutionBinding, NodeIdx};
use crate::execution::{ArgumentValues, ExecutionEngine};
use crate::execution_stats::ExecutionStats;
use crate::graph::NodeId;

impl ExecutionEngine {
    /// Resident-only argument values. The public entry point is
    /// [`Self::get_argument_values_with_previews`], which hydrates disk-cached
    /// values first; this bare accessor reads whatever is in RAM, so a disk-only
    /// node reads back empty. Kept `pub(crate)` to keep that footgun off the public
    /// surface.
    pub(crate) fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
        let idx: NodeIdx = self.program.e_nodes.index_of_key(node_id)?.into();
        let e_node = &self.program.e_nodes[idx];

        let inputs = self.program.inputs[e_node.inputs.range()]
            .iter()
            .map(|input| match &input.binding {
                ExecutionBinding::None => None,
                ExecutionBinding::Const(v) => Some(DynamicValue::from(v)),
                ExecutionBinding::Bind(addr) => self
                    .cache
                    .output_values(addr.target_idx)
                    .and_then(|o| o.get(addr.port_idx))
                    .cloned(),
            })
            .collect();

        let outputs = self
            .cache
            .output_values(idx)
            .map(|o| o.to_vec())
            .unwrap_or_default();

        Some(ArgumentValues { inputs, outputs })
    }

    /// `get_argument_values` plus awaited preview resolution. Reads any disk-cached
    /// value this node shows (its own outputs and its inputs' producers) into RAM
    /// first — `mark_available` flags them without loading, so an inspected node a
    /// run never touched still resolves.
    pub async fn get_argument_values_with_previews(
        &mut self,
        node_id: &NodeId,
    ) -> Option<ArgumentValues> {
        if let Some(idx) = self.program.e_nodes.index_of_key(node_id) {
            self.output_cache
                .hydrate_for_inspection(&self.program, &mut self.cache, idx.into());
        }
        let mut values = self.get_argument_values(node_id)?;
        let mut pending_previews = Vec::new();
        for value in values
            .inputs
            .iter_mut()
            .flatten()
            .chain(values.outputs.iter_mut())
        {
            if let Some(pending) = value.gen_preview(&mut self.executor.ctx_manager) {
                pending_previews.push(pending);
            }
        }
        for pending in pending_previews {
            pending.wait(&mut self.executor.ctx_manager).await;
        }
        Some(values)
    }

    /// Collect every (event → lambda → state) triple that is currently "live" —
    /// node was executed or cached this run, the event has at least one
    /// subscriber, and its lambda is populated. Used by the worker to spawn the
    /// tasks that drive the event loop.
    pub fn active_event_triggers(&self, stats: &ExecutionStats) -> Vec<EventTrigger> {
        stats
            .cached_nodes
            .iter()
            .copied()
            .chain(stats.executed_nodes.iter().map(|n| n.node_id))
            .flat_map(|node_id| {
                let e_node = self.by_id(&node_id).unwrap();
                let event_state = self
                    .cache
                    .slots
                    .by_key(&node_id)
                    .unwrap()
                    .event_state
                    .clone();
                let id = e_node.id;
                self.program.events[e_node.events.range()]
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| !event.subscribers.is_empty() && !event.lambda.is_none())
                    .map(move |(event_idx, event)| EventTrigger {
                        event: EventRef {
                            node_id: id,
                            event_idx,
                        },
                        lambda: event.lambda.clone(),
                        state: event_state.clone(),
                    })
            })
            .collect()
    }
}
