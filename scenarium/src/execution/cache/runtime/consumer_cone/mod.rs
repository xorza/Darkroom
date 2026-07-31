//! Downstream cones over a program's data edges.

use crate::common::set::IdxSet;
use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionBinding};
use crate::execution::identity::NodeIdx;

/// What a set of seed nodes reaches downstream: every node that reads a seed's
/// outputs, transitively, plus the seeds themselves.
///
/// Answering that needs the program's data edges reversed, which is a pure
/// function of the program and so is **derived per call, never kept** — the one
/// question that needs it, what evicting a node reaches, is a user action,
/// while an index held between calls would be stale the moment the graph is
/// edited. What persists is the *storage*: [`RuntimeCache`] owns one of these,
/// so the second eviction refills the buffers below instead of allocating them
/// again.
///
/// The reversed edges are one packed run per node rather than a `Vec` per node.
/// The walk visits one node after another, so a run only ever has to be a
/// slice, and the whole structure is four flat allocations instead of one per
/// node.
///
/// [`RuntimeCache`]: crate::execution::cache::runtime::RuntimeCache
#[derive(Debug, Default)]
pub(crate) struct ConsumerCone {
    /// Reader runs, concatenated in node order.
    readers: Vec<NodeIdx>,
    /// `starts[i]..starts[i + 1]` is node `i`'s run, so this holds one more
    /// entry than the program has nodes.
    starts: Vec<u32>,
    /// The cursor the placement pass walks each run with, each ending where the
    /// next run begins — which is what makes that pass a placement rather than
    /// a search.
    filled: Vec<u32>,
    /// The walk's frontier.
    pending: Vec<NodeIdx>,
    /// The walk's answer, doubling as its visited mark — so a seed named twice,
    /// and a node two consumers reach, each cost one visit.
    reached: IdxSet<NodeIdx>,
}

impl ConsumerCone {
    /// The reflexive transitive closure of `seeds` over `program`'s reversed
    /// data edges.
    ///
    /// Handed back in the dense index space rather than as ids: the caller
    /// walks the same space, and converting here would only make it convert
    /// back. Borrowed from `self`, so it lives until the next call.
    pub(crate) fn of(
        &mut self,
        program: &CompiledGraph,
        seeds: impl IntoIterator<Item = NodeIdx>,
    ) -> &IdxSet<NodeIdx> {
        self.reverse(program);
        self.walk(seeds);
        &self.reached
    }

    /// Reverse every data edge in `program` into `readers`/`starts`.
    ///
    /// Counting sort rather than a comparison sort: the key is a dense index,
    /// so one pass tallies each node's readers and a second places them.
    fn reverse(&mut self, program: &CompiledGraph) {
        let ConsumerCone {
            readers,
            starts,
            filled,
            ..
        } = self;
        let node_count = program.e_nodes.len();
        starts.clear();
        starts.resize(node_count + 1, 0);
        for e_node in program.e_nodes.iter() {
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    starts[address.node_idx.0 as usize + 1] += 1;
                }
            }
        }
        for at in 0..node_count {
            starts[at + 1] += starts[at];
        }

        readers.clear();
        readers.resize(starts[node_count] as usize, NodeIdx(0));
        filled.clear();
        filled.extend_from_slice(starts);
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    let at = &mut filled[address.node_idx.0 as usize];
                    readers[*at as usize] = node_idx;
                    *at += 1;
                }
            }
        }
    }

    /// Breadth of the cone, from the reversed edges [`reverse`](Self::reverse)
    /// just laid down.
    fn walk(&mut self, seeds: impl IntoIterator<Item = NodeIdx>) {
        let ConsumerCone {
            readers,
            starts,
            pending,
            reached,
            ..
        } = self;
        reached.reset(starts.len() - 1);
        pending.clear();
        for node_idx in seeds {
            if !reached.contains(node_idx) {
                reached.insert(node_idx);
                pending.push(node_idx);
            }
        }
        while let Some(node_idx) = pending.pop() {
            let at = node_idx.0 as usize;
            for &consumer_idx in &readers[starts[at] as usize..starts[at + 1] as usize] {
                if !reached.contains(consumer_idx) {
                    reached.insert(consumer_idx);
                    pending.push(consumer_idx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
