//! [`Runs`]: the executor over a hand-built program, held across runs so the
//! second sees what the first left resident.

use common::CancelToken;

use crate::DynamicValue;
use crate::RamUsage;
use crate::containers::column::Column;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::execution::error::RunError;
use crate::execution::executor::{Executor, RunRequest};
use crate::execution::identity::OutputAddr;
use crate::execution::report::ExecutionOutcome;
use crate::execution::report::internals::DiscardedReports;
use crate::execution::schedule::{NodeState, ResolvedOutputs, RootFlags, RunSchedule};
use crate::graph::func::lambda::OutputDemand;
use crate::testing::program::{Placed, ProgramBuilder};

/// The executor over a hand-built program, keeping its cache and run loop
/// across [`go`](Self::go) — so a second run sees what the first left resident,
/// which is what a reuse hit is made of.
///
/// **The run is stated, not planned.** By default every node runs with one
/// reader per output, which is what an executor test wants: the verdicts a
/// sweep would write are [`Sweep`]'s subject, and re-deriving them here would
/// make every assertion depend on two passes instead of one.
/// [`resolved`](Self::resolved) opts back into the sweep for the fixtures where
/// what the *second* run decides is the answer.
///
/// Everything reads back by [`Placed`]: what a node's slot holds, and what the
/// outcome says it did.
#[derive(Debug)]
pub(crate) struct Runs<'a> {
    owner: &'a ProgramBuilder,
    schedule: RunSchedule,
    cache: RuntimeCache,
    executor: Executor,
    outcome: ExecutionOutcome,
    resolve: bool,
    cancel: CancelToken,
}

impl<'a> Runs<'a> {
    pub(super) fn new(owner: &'a ProgramBuilder) -> Self {
        let mut schedule = owner.staged(NodeState::Run);
        for placed in &owner.placed {
            schedule.add_root(placed.node_idx, RootFlags::PLAIN);
        }
        let mut cache = RuntimeCache::default();
        cache.install_for_test(&owner.program);
        let mut runs = Self {
            owner,
            schedule,
            cache,
            executor: Executor::default(),
            outcome: ExecutionOutcome::default(),
            resolve: false,
            cancel: CancelToken::never(),
        };
        let one_each = vec![1; runs.program().outputs.len()];
        runs.set_readers(one_each);
        runs
    }

    /// Per-output live-reader counts, replacing the default one each. An output
    /// nothing reads is `Skip`, which is how a fixture spells a sink — released
    /// the instant it runs — or claims more readers than really read, to prove
    /// the release waits for the full count.
    pub(crate) fn readers(mut self, counts: impl IntoIterator<Item = u32>) -> Self {
        self.set_readers(counts.into_iter().collect());
        self
    }

    /// Re-derive dispositions, demand, and reader counts from the cache before
    /// every run, the way the engine does.
    ///
    /// The sweep overwrites whatever [`readers`](Self::readers) stated, so the
    /// two do not combine.
    pub(crate) fn resolved(mut self) -> Self {
        self.resolve = true;
        self.schedule = self.owner.planned();
        self
    }

    /// Override one node's disposition — a `Reuse` the fixture states rather
    /// than provokes, or a node the planner would have blocked.
    pub(crate) fn state(mut self, node: Placed, state: NodeState) -> Self {
        self.schedule.states[node.node_idx] = state;
        self
    }

    /// Demand one output no consumer reads — what a node seed does, and the
    /// only way `Produce` reaches a port with a zero reader count.
    pub(crate) fn demand(mut self, node: Placed, port: usize) -> Self {
        let output_idx = self.program().output_idx(OutputAddr {
            node_idx: node.node_idx,
            port_idx: port as u32,
        });
        self.schedule.outputs.demand[output_idx] = OutputDemand::Produce;
        self
    }

    /// Make `node` the run's one root — for a fixture about what a walk
    /// starting somewhere other than "everything" reaches.
    pub(crate) fn only_root(mut self, node: Placed) -> Self {
        self.schedule.clear_roots();
        self.schedule.add_root(node.node_idx, RootFlags::PLAIN);
        self
    }

    /// Mark `node` a node-seeded root: every output demanded, `disabled`
    /// overridden.
    pub(crate) fn seeded(mut self, node: Placed) -> Self {
        self.schedule.add_root(node.node_idx, RootFlags::SEEDED);
        self
    }

    /// Prime the cache: `values` resident under the digest this run stamps for
    /// `node`, so it reads as a hit.
    pub(crate) fn cached(
        mut self,
        node: Placed,
        values: impl IntoIterator<Item = DynamicValue>,
    ) -> Self {
        let Runs {
            owner,
            schedule,
            cache,
            ..
        } = &mut self;
        cache.stamp_digests(&owner.program, schedule.executing());
        let digest = cache[node.node_idx]
            .current_digest
            .expect("a cached fixture node is reproducible, so it has a digest");
        cache[node.node_idx].load_output(
            OutputSnapshot::new(values.into_iter().collect()),
            Some(digest),
        );
        self
    }

    /// Leave `values` in `node`'s slot under **no** digest — a stale value from
    /// some earlier run, which nothing this run may serve.
    pub(crate) fn resident(
        mut self,
        node: Placed,
        values: impl IntoIterator<Item = DynamicValue>,
    ) -> Self {
        self.cache[node.node_idx]
            .load_output(OutputSnapshot::new(values.into_iter().collect()), None);
        self
    }

    /// Run under `cancel` rather than a token nothing trips.
    pub(crate) fn cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Walk the schedule once, keeping the cache for the next call.
    pub(crate) async fn go(&mut self) {
        let Runs {
            owner,
            schedule,
            cache,
            executor,
            outcome,
            resolve,
            cancel,
        } = self;
        let program = &owner.program;
        if *resolve {
            schedule.resolve(program, cache).await;
        }
        executor
            .run(
                RunRequest {
                    program,
                    schedule,
                    cache,
                    reporter: &mut DiscardedReports,
                    cancel: cancel.clone(),
                },
                outcome,
            )
            .await;
        // No RAM is measured: filling that column is the engine's post-run
        // release sweep, which is not part of the run loop under test.
        let mut node_ram = Column::default();
        node_ram.reset(program.e_nodes.len(), RamUsage::default());
        executor.collect_outcome(program, schedule, &node_ram, outcome);
    }

    pub(crate) fn program(&self) -> &CompiledGraph {
        &self.owner.program
    }

    /// What `node`'s slot holds — `None` once a release has reclaimed it.
    pub(crate) fn outputs(&self, node: Placed) -> Option<&[DynamicValue]> {
        self.cache[node.node_idx].output_values()
    }

    /// `node`'s output `port` read as an integer. `DynamicValue` is not
    /// `PartialEq`, so a scalar assertion goes through the same coercion a
    /// consuming lambda would.
    pub(crate) fn output_i64(&self, node: Placed, port: usize) -> Option<i64> {
        self.outputs(node)?.get(port)?.as_i64()
    }

    /// Whether `node` invoked its lambda and succeeded.
    pub(crate) fn ran(&self, node: Placed) -> bool {
        self.outcome.ran(node.node_id)
    }

    /// Whether `node` was served from a cache instead of recomputing.
    pub(crate) fn reused(&self, node: Placed) -> bool {
        self.outcome.cached(node.node_id)
    }

    pub(crate) fn error(&self, node: Placed) -> Option<&RunError> {
        self.outcome.error(node.node_id)
    }

    pub(crate) fn ran_count(&self) -> usize {
        self.outcome.ran_node_count
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.outcome.cancelled
    }

    /// Planned reads of `node`'s output `port` the run has not completed — zero
    /// once every consumer has read it or had its read retired.
    pub(crate) fn remaining_reads(&self, node: Placed, port: usize) -> u32 {
        let output_idx = self.program().output_idx(OutputAddr {
            node_idx: node.node_idx,
            port_idx: port as u32,
        });
        self.executor.remaining_reads(output_idx)
    }

    fn set_readers(&mut self, readers: Vec<u32>) {
        assert_eq!(
            readers.len(),
            self.program().outputs.len(),
            "one reader count per output in the program's pool",
        );
        let demand = readers
            .iter()
            .map(|count| {
                if *count == 0 {
                    OutputDemand::Skip
                } else {
                    OutputDemand::Produce
                }
            })
            .collect::<Vec<_>>();
        self.schedule.outputs = ResolvedOutputs {
            demand: Column::from(demand),
            readers: Column::from(readers),
        };
    }
}
