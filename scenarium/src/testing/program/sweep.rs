//! [`Sweep`]: the cache-aware pass over a schedule a fixture stated by hand,
//! and [`Swept`], what it leaves behind.

use crate::DynamicValue;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::execution::schedule::{NodeState, RootFlags, RunSchedule};
use crate::graph::func::lambda::OutputDemand;
use crate::testing::program::{Placed, ProgramBuilder};

/// The cache-aware sweep over a hand-stated schedule: which nodes the planner
/// would have rooted, which it would have blocked, and what the cache already
/// holds.
#[derive(Debug)]
pub(crate) struct Sweep<'a> {
    owner: &'a ProgramBuilder,
    roots: Vec<(Placed, RootFlags)>,
    missing: Vec<Placed>,
    cached: Vec<(Placed, Vec<DynamicValue>)>,
}

impl<'a> Sweep<'a> {
    pub(super) fn new(owner: &'a ProgramBuilder) -> Self {
        Self {
            owner,
            roots: Vec::new(),
            missing: Vec::new(),
            cached: Vec::new(),
        }
    }

    /// A plain walk root — a sink, or an event subscriber.
    pub(crate) fn root(mut self, node: Placed) -> Self {
        self.roots.push((node, RootFlags::PLAIN));
        self
    }

    /// A node-seeded root: every output demanded, `disabled` overridden.
    pub(crate) fn seeded(mut self, node: Placed) -> Self {
        self.roots.push((node, RootFlags::SEEDED));
        self
    }

    /// A node the planner blocked for want of a required input.
    pub(crate) fn missing(mut self, node: Placed) -> Self {
        self.missing.push(node);
        self
    }

    /// Prime the cache: `values` resident under the digest the sweep is about
    /// to stamp for `node`, so it reads as a hit.
    pub(crate) fn cached(
        mut self,
        node: Placed,
        values: impl IntoIterator<Item = DynamicValue>,
    ) -> Self {
        self.cached.push((node, values.into_iter().collect()));
        self
    }

    /// Run the sweep, handing back the schedule it refined and the cache it
    /// stamped.
    pub(crate) async fn run(self) -> Swept<'a> {
        let Sweep {
            owner,
            roots,
            missing,
            cached,
        } = self;
        let program = &owner.program;

        // `Cut` is the planner's positive verdict — runnable, nothing claimed
        // it yet — so that is what a swept-from schedule starts at.
        let mut schedule = owner.staged(NodeState::Cut);
        for node in missing {
            schedule.states[node.node_idx] = NodeState::MissingInputs;
        }
        for (node, flags) in roots {
            schedule.add_root(node.node_idx, flags);
        }

        let mut cache = RuntimeCache::default();
        cache.install_for_test(program);
        cache.stamp_digests(program, schedule.executing());
        for (node, values) in cached {
            let digest = cache[node.node_idx]
                .current_digest
                .expect("a cached fixture node is reproducible, so it has a digest");
            cache[node.node_idx].load_output(OutputSnapshot::new(values), Some(digest));
        }

        schedule.resolve(program, &mut cache).await;
        Swept { program, schedule }
    }
}

/// What a [`Sweep`] left behind, over the program it swept — so an assertion
/// names a node's port run without the caller re-deriving it.
#[derive(Debug)]
pub(crate) struct Swept<'a> {
    program: &'a CompiledGraph,
    pub(crate) schedule: RunSchedule,
}

impl Swept<'_> {
    pub(crate) fn state(&self, node: Placed) -> NodeState {
        self.schedule.states[node.node_idx]
    }

    pub(crate) fn demand(&self, node: Placed) -> &[OutputDemand] {
        &self.schedule.outputs.demand[self.program[node.node_idx].outputs]
    }

    pub(crate) fn readers(&self, node: Placed) -> &[u32] {
        &self.schedule.outputs.readers[self.program[node.node_idx].outputs]
    }
}
