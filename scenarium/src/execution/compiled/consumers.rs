//! Data edges reversed, derived where they are asked for.

use crate::execution::compiled::{CompiledGraph, ExecutionBinding};
use crate::execution::identity::NodeIdx;

/// Which nodes read each node's outputs — the [`CompiledGraph`]'s data edges, walked
/// backwards.
///
/// A pure function of the program, so it is *derived* rather than kept: the two
/// questions that need it — what a "run this node" seeds, what evicting one
/// reaches — are user actions, while an artifact holding it would be rebuilt on
/// every edit. Building it costs one pass over the input pool; keeping it cost
/// a sort per compile.
///
/// Stored as one packed run per node rather than a `Vec` per node: both readers
/// walk one node after another, and the whole structure lives no longer than
/// the query that built it.
#[derive(Debug)]
pub(crate) struct Consumers {
    /// Reader runs, concatenated in node order.
    readers: Vec<NodeIdx>,
    /// `starts[i]..starts[i + 1]` is node `i`'s run, so this holds one more
    /// entry than the program has nodes.
    starts: Vec<u32>,
}

impl Consumers {
    /// Reverse every data edge in `program`.
    ///
    /// Counting sort rather than a comparison sort: the key is a dense index,
    /// so one pass tallies each node's readers and a second places them.
    pub(crate) fn reverse(program: &CompiledGraph) -> Self {
        let node_count = program.e_nodes.len();
        let mut starts = vec![0u32; node_count + 1];
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

        let mut readers = vec![NodeIdx(0); starts[node_count] as usize];
        // `filled` walks each run as it is placed, ending where the next run
        // begins — which is what makes the second pass a placement rather than
        // a search.
        let mut filled = starts.clone();
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    let at = &mut filled[address.node_idx.0 as usize];
                    readers[*at as usize] = node_idx;
                    *at += 1;
                }
            }
        }
        Consumers { readers, starts }
    }

    /// The nodes reading `node_idx`'s outputs. Empty when nothing does.
    pub(crate) fn of(&self, node_idx: NodeIdx) -> &[NodeIdx] {
        let at = node_idx.0 as usize;
        &self.readers[self.starts[at] as usize..self.starts[at + 1] as usize]
    }
}
