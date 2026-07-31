//! Data edges reversed, derived where they are asked for.

use crate::common::set::IdxSet;
use crate::execution::compiled::{CompiledGraph, ExecutionBinding};
use crate::execution::identity::NodeIdx;

/// Which nodes read each node's outputs — the [`CompiledGraph`]'s data edges, walked
/// backwards.
///
/// A pure function of the program, so it is *derived* rather than kept: the one
/// question that needs it — what evicting a node reaches — is a user action,
/// while an artifact holding it would be rebuilt on every edit. Building it
/// costs one pass over the input pool; keeping it cost a sort per compile.
///
/// Stored as one packed run per node rather than a `Vec` per node: the walk
/// visits one node after another, and the whole structure lives no longer than
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

    /// The reflexive transitive closure of `seeds` over these edges: every node
    /// that reads a seed's outputs, transitively, plus the seeds themselves.
    ///
    /// The set *is* the visited mark, so a seed named twice and a node two
    /// consumers reach each cost one visit. It is handed back as-is rather than
    /// collected: the caller walks the same dense space, and converting to ids
    /// here would only make it convert them back.
    pub(crate) fn closure(&self, seeds: impl IntoIterator<Item = NodeIdx>) -> IdxSet<NodeIdx> {
        let mut reached = IdxSet::default();
        reached.reset(self.starts.len() - 1);
        let mut pending = Vec::new();
        for node_idx in seeds {
            if !reached.contains(node_idx) {
                reached.insert(node_idx);
                pending.push(node_idx);
            }
        }
        while let Some(node_idx) = pending.pop() {
            for &consumer_idx in self.of(node_idx) {
                if !reached.contains(consumer_idx) {
                    reached.insert(consumer_idx);
                    pending.push(consumer_idx);
                }
            }
        }
        reached
    }

    /// The nodes reading `node_idx`'s outputs. Empty when nothing does.
    fn of(&self, node_idx: NodeIdx) -> &[NodeIdx] {
        let at = node_idx.0 as usize;
        &self.readers[self.starts[at] as usize..self.starts[at + 1] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::compile::Compiler;
    use crate::graph::identity::{InputPort, NodeId};
    use crate::graph::{Binding, Graph};
    use crate::testing::{TestFuncHooks, test_func_lib};

    /// A sink reading a source, plus an unwired node — the three positions a
    /// seed can occupy relative to a consumer edge.
    struct Fixture {
        program: CompiledGraph,
        source: NodeId,
        sink: NodeId,
        loose: NodeId,
    }

    fn fixture() -> Fixture {
        let library = test_func_lib(TestFuncHooks::default());
        let mut graph = Graph::default();
        let source = graph.add_func_node(library.by_name("get_a").unwrap());
        let sink = graph.add_func_node(library.by_name("Print").unwrap());
        let loose = graph.add_func_node(library.by_name("get_b").unwrap());
        graph.set_input_binding(InputPort::new(sink, 0), Binding::bind(source, 0));
        Fixture {
            program: Compiler::default().compile(&graph, &library).unwrap(),
            source,
            sink,
            loose,
        }
    }

    /// The closure back in the id space, so an assertion names nodes rather than
    /// wherever the id sort happened to place them.
    fn reached(f: &Fixture, seeds: &[NodeId]) -> Vec<NodeId> {
        Consumers::reverse(&f.program)
            .closure(
                seeds
                    .iter()
                    .filter_map(|node_id| f.program.node_index.get(node_id).copied()),
            )
            .iter()
            .map(|node_idx| f.program.node_ids[node_idx])
            .collect()
    }

    /// A seed reaches everything downstream of it, reflexively — and stops at a
    /// node nothing connects to.
    #[test]
    fn the_closure_reaches_downstream_and_stops() {
        let f = fixture();
        let mut expected = vec![f.source, f.sink];
        expected.sort();
        assert_eq!(
            reached(&f, &[f.source]),
            expected,
            "a source reaches its reader"
        );
        assert_eq!(
            reached(&f, &[f.sink]),
            vec![f.sink],
            "a terminal node reaches only itself"
        );
        assert_eq!(
            reached(&f, &[f.loose]),
            vec![f.loose],
            "an unwired node reaches only itself"
        );
        assert!(
            reached(&f, &[NodeId::unique()]).is_empty(),
            "an id the program never held seeds nothing"
        );
    }

    /// The set is the visited mark, so a seed named twice — and a seed already
    /// reachable from another — appears exactly once.
    #[test]
    fn repeated_and_overlapping_seeds_are_visited_once() {
        let f = fixture();
        let mut expected = vec![f.source, f.sink];
        expected.sort();
        assert_eq!(reached(&f, &[f.source, f.source]), expected);
        assert_eq!(reached(&f, &[f.source, f.sink]), expected);
    }

    /// Ascending indices out, and the walk assigns them in id order — so the
    /// caller reads the cone in a stable order across compiles of one graph.
    #[test]
    fn the_closure_walks_the_dense_space_in_order() {
        let f = fixture();
        let all = reached(&f, &[f.source, f.sink, f.loose]);
        assert_eq!(all.len(), 3);
        assert!(all.is_sorted());
    }
}
