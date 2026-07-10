//! Read-only projections off a compiled [`ExecutionEngine`]: per-node argument
//! values (optionally with awaited previews) and the live event triggers the
//! worker spawns to drive the event loop. These read the program and cache only;
//! they never schedule or run.

use crate::data::DynamicValue;
use crate::execution::event::{EventRef, EventTrigger};
use crate::execution::program::{ExecutionBinding, NodeIdx};
use crate::execution::stats::ExecutionStats;
use crate::execution::{ArgumentValues, ExecutionEngine};
use crate::graph::NodeId;

/// The flat execution node backing authoring id `node_id`. A top-level node keeps its
/// authoring id; a subgraph-interior node's flat id is hashed from the descent path, so
/// the authoring id misses the key lookup — resolve it through the flatten map's leaves
/// instead. A def instanced more than once has several flat nodes per interior id; the
/// first (lowest index) is returned, an arbitrary-but-stable representative for
/// inspection and node seeding.
pub(crate) fn resolve_node_idx(
    program: &ExecutionProgram,
    flatten: &FlattenMap,
    node_id: &NodeId,
) -> Option<NodeIdx> {
    if let Some(idx) = program.e_nodes.index_of_key(node_id) {
        return Some(idx.into());
    }
    program
        .node_indices()
        .find(|&idx| flatten.interior(program.e_nodes[idx].id) == Some(*node_id))
}

impl ExecutionEngine {
    /// Resident-only argument values, test inspection only. The production entry
    /// point is [`Self::get_argument_values_with_previews`], which hydrates
    /// disk-cached values first; this bare accessor reads whatever is in RAM, so a
    /// disk-only node reads back empty.
    #[cfg(test)]
    pub(crate) fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
        let idx = resolve_node_idx(&self.program, &self.flatten_map, node_id)?;
        Some(self.argument_values_at(idx))
    }

    fn argument_values_at(&self, idx: NodeIdx) -> ArgumentValues {
        let e_node = &self.program.e_nodes[idx];

        let inputs = self.program.inputs[e_node.inputs.range()]
            .iter()
            .map(|input| match &input.binding {
                ExecutionBinding::None => None,
                ExecutionBinding::Const(v) => Some(DynamicValue::from(v)),
                ExecutionBinding::Bind(addr) => self.cache.slots[addr.target_idx]
                    .output_values()
                    .and_then(|o| o.get(addr.port_idx))
                    .cloned(),
            })
            .collect();

        let outputs = self.cache.slots[idx]
            .output_values()
            .map(|o| o.to_vec())
            .unwrap_or_default();

        ArgumentValues { inputs, outputs }
    }

    /// `get_argument_values` plus awaited preview resolution. Reads any disk-cached
    /// value this node shows (its own outputs and its inputs' producers) into RAM first
    /// via [`hydrate_for_inspection`](crate::execution::cache::RuntimeCache::hydrate_for_inspection),
    /// so an inspected node the run reused-from-disk (or never touched) still resolves.
    /// The hydration is a loan: once the values are cloned out (the reply's `Arc`s keep
    /// the data alive), the loaned slots go back to `OnDisk` — inspection must not leave
    /// resident what the node's mode wouldn't retain, since the reuse check trusts
    /// residency.
    pub(crate) async fn get_argument_values_with_previews(
        &mut self,
        node_id: &NodeId,
    ) -> Option<ArgumentValues> {
        let idx = self.resolve_query_idx(node_id)?;
        self.cache.hydrate_for_inspection(&self.program, idx).await;
        let mut values = self.argument_values_at(idx);
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
    pub(crate) fn active_event_triggers(&self, stats: &ExecutionStats) -> Vec<EventTrigger> {
        stats
            .cached_nodes
            .iter()
            .copied()
            .chain(stats.executed_nodes.iter().map(|n| n.node_id))
            .flat_map(|node_id| {
                // One key lookup: `e_nodes` and `cache.slots` are index-aligned.
                let idx = self.program.e_nodes.index_of_key(&node_id).unwrap();
                let e_node = &self.program.e_nodes[idx];
                let event_state = self.cache.slots[idx].event_state.clone();
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
