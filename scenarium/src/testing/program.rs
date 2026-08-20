//! [`ProgramBuilder`]: a [`CompiledGraph`] built by hand, for the tests below
//! the compiler.
//!
//! The schedule and cache tests are about topology, not authoring — they want
//! "a source, a consumer reading its port 0, a sink" and no library at all, so
//! they hand-build the artifact.
//!
//! The id↔index pair is a value ([`Placed`]) rather than a convention each
//! fixture re-derives, and a binding is asked of the node it points at
//! ([`Placed::out`]).
//!
//! The two terminals are [`Sweep`] — what the cache-aware pass makes of a
//! schedule — and [`Runs`], the executor over one. A sweep asks what the
//! schedule *became*; a run asks what walking it *did*.
//!
//! The compiler's own tests keep using the real [`Compiler`](crate::Compiler):
//! this is for programs a compile *wouldn't* produce — a corrupt artifact a
//! validator must reject, or a topology that would take a library to express.

use common::CancelToken;

use crate::RamUsage;
use crate::containers::column::Column;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::compile::compiled_graph::{
    CompiledGraph, ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode,
};
use crate::execution::error::{Result, RunError};
use crate::execution::executor::{Executor, RunRequest};
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::execution::report::ExecutionOutcome;
use crate::execution::report::internals::DiscardedReports;
use crate::execution::schedule::planner::Planner;
use crate::execution::schedule::{NodeState, ResolvedOutputs, RootFlags, RunSchedule};
use crate::execution::seeds::RunSeeds;
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::{FuncLambda, OutputDemand};
use crate::graph::identity::{FuncId, NodeId};
use crate::graph::node::CacheMode;
use crate::{ConstValue, DataType, DynamicValue, async_lambda};

/// Where one node landed: the stable id a host names it by, and the dense
/// index every per-run column is keyed on.
///
/// Carrying both is what removes the `NodeId::from_u128(idx + 1)` convention —
/// a fixture asks the node rather than re-deriving one from the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) node_id: NodeId,
    pub(crate) node_idx: NodeIdx,
}

impl Placed {
    /// A binding reading this node's output `port`.
    pub(crate) fn out(self, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(self.addr(port))
    }

    /// This node's output `port` as the interned address a binding carries —
    /// for a fixture that *names* a port rather than reads through one.
    pub(crate) fn addr(self, port: usize) -> OutputAddr {
        OutputAddr {
            node_idx: self.node_idx,
            port_idx: port as u32,
        }
    }
}

/// Builds a [`CompiledGraph`] node by node, in the ascending id order a real
/// compile places them in.
#[derive(Debug, Default)]
pub(crate) struct ProgramBuilder {
    program: CompiledGraph,
    placed: Vec<Placed>,
}

impl ProgramBuilder {
    /// Open a node. Nothing lands until [`NodeBuilder::add`].
    pub(crate) fn node(&mut self) -> NodeBuilder<'_> {
        NodeBuilder {
            owner: self,
            e_node: ExecutionNode::default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: 0,
            node_id: None,
        }
    }

    pub(crate) fn program(&self) -> &CompiledGraph {
        &self.program
    }

    /// The finished artifact, for a test that installs it rather than planning
    /// over it.
    pub(crate) fn into_program(self) -> CompiledGraph {
        self.program
    }

    pub(crate) fn program_mut(&mut self) -> &mut CompiledGraph {
        &mut self.program
    }

    /// Plan `seeds` over the program, surfacing the planner's refusal — a
    /// dependency cycle, or a seed the program does not hold.
    pub(crate) fn try_plan(&self, seeds: &RunSeeds) -> Result<RunSchedule> {
        let mut schedule = RunSchedule::default();
        Planner::default().plan(&self.program, seeds, &mut schedule)?;
        Ok(schedule)
    }

    /// [`try_plan`](Self::try_plan) for the majority of tests, where planning
    /// succeeding is a precondition rather than the subject.
    pub(crate) fn plan(&self, seeds: &RunSeeds) -> RunSchedule {
        self.try_plan(seeds).expect("the fixture program plans")
    }

    /// Plan every sink — the ordinary "produce the outputs" run.
    pub(crate) fn plan_sinks(&self) -> RunSchedule {
        self.plan(&RunSeeds::sinks())
    }

    /// Stand up the cache-aware sweep over a schedule the fixture describes
    /// directly, rather than one the planner produced.
    ///
    /// Sweep tests are about what liveness and reuse do to an *already*
    /// planned schedule, so they state the planner's output — roots, blocked
    /// nodes — instead of arranging a graph that would provoke it.
    pub(crate) fn sweep(&self) -> Sweep<'_> {
        Sweep {
            owner: self,
            roots: Vec::new(),
            missing: Vec::new(),
            cached: Vec::new(),
        }
    }

    /// Stand up the executor over this program — see [`Runs`].
    pub(crate) fn runs(&self) -> Runs<'_> {
        Runs::new(self)
    }

    /// The planner's output, stated rather than provoked: every placed node in
    /// `process_order`, each verdicted `initial`, and no roots yet.
    ///
    /// The one place the "a hand-built program's nodes run in placement order"
    /// convention lives, so the sweep and the run loop cannot disagree about
    /// what a fixture's schedule looks like.
    fn staged(&self, initial: NodeState) -> RunSchedule {
        let mut schedule = RunSchedule::default();
        schedule.reset_for_program(&self.program);
        schedule
            .process_order
            .extend(self.placed.iter().map(|placed| placed.node_idx));
        schedule.states.reset(self.program.e_nodes.len(), initial);
        schedule
    }

    /// [`staged`](Self::staged) at the planner's positive verdict with every
    /// node a plain root — the structural plan a whole-program run starts from.
    ///
    /// For a fixture driving a pass *below* the planner, where arranging a
    /// graph that would provoke this plan says nothing the test is about.
    pub(crate) fn planned(&self) -> RunSchedule {
        let mut schedule = self.staged(NodeState::Cut);
        for placed in &self.placed {
            schedule.add_root(placed.node_idx, RootFlags::PLAIN);
        }
        schedule
    }
}

/// One node under construction. Ports are accumulated here and packed into the
/// program's columns by [`add`](Self::add), so a node's runs are contiguous the
/// way a compile leaves them.
#[derive(Debug)]
pub(crate) struct NodeBuilder<'a> {
    owner: &'a mut ProgramBuilder,
    e_node: ExecutionNode,
    inputs: Vec<ExecutionInput>,
    outputs: Vec<DataType>,
    events: usize,
    node_id: Option<NodeId>,
}

impl NodeBuilder<'_> {
    pub(crate) fn sink(mut self) -> Self {
        self.e_node.sink = true;
        self
    }

    pub(crate) fn pure(mut self) -> Self {
        self.e_node.behavior = FuncBehavior::Pure;
        self
    }

    pub(crate) fn cache(mut self, mode: CacheMode) -> Self {
        self.e_node.cache = mode;
        self
    }

    /// Override the implementation this node's slot is owned by. Placement
    /// gives each node a distinct one; a test names it only when reinstalling
    /// a node under the *same* implementation, which is what keeps its state.
    pub(crate) fn func(mut self, func_id: FuncId) -> Self {
        self.e_node.func_id = func_id;
        self
    }

    /// Override the stable id this node is placed under.
    ///
    /// Placement numbers nodes from 1 in order, which is all a fixture needs
    /// until it reinstalls a *different* set of nodes — where the point is that
    /// the surviving ids keep their slots while the index space shifts under
    /// them. Ids must still ascend, since `node_ids` is binary-searched.
    pub(crate) fn id(mut self, node_id: NodeId) -> Self {
        self.node_id = Some(node_id);
        self
    }

    pub(crate) fn lambda(mut self, lambda: FuncLambda) -> Self {
        self.e_node.lambda = lambda;
        self
    }

    /// A no-op body, for a node the sweep or the run loop must treat as
    /// runnable rather than `MissingLambda`.
    pub(crate) fn stub(self) -> Self {
        self.lambda(async_lambda!(|_| { Ok(()) }))
    }

    /// A node a cache could serve: content-cacheable, so it earns a digest, and
    /// implemented, so the sweep does not verdict it `MissingLambda` first.
    ///
    /// The two together are what "could this be reused" takes, which is why
    /// every sweep fixture states them.
    pub(crate) fn reusable(self) -> Self {
        self.pure().stub()
    }

    /// An optional input reading `binding`.
    pub(crate) fn input(self, binding: ExecutionBinding) -> Self {
        self.push_input(ExecutionInput {
            required: false,
            stamps_fs_path: false,
            binding,
        })
    }

    /// A required input reading `binding` — [`ExecutionBinding::None`] for the
    /// unbound case the planner verdicts `MissingInputs`.
    pub(crate) fn required(self, binding: ExecutionBinding) -> Self {
        self.push_input(ExecutionInput {
            required: true,
            stamps_fs_path: false,
            binding,
        })
    }

    /// An optional input carrying a literal.
    pub(crate) fn const_input(self, value: impl Into<ConstValue>) -> Self {
        self.input(ExecutionBinding::Const(value.into()))
    }

    /// An input whose delivered value's filesystem referent folds into this
    /// node's digest — what the compiler writes for an `FsPath`-declared port,
    /// and the only thing that makes a path re-key its consumer.
    pub(crate) fn fs_path_input(self, binding: ExecutionBinding) -> Self {
        self.push_input(ExecutionInput {
            required: false,
            stamps_fs_path: true,
            binding,
        })
    }

    fn push_input(mut self, input: ExecutionInput) -> Self {
        self.inputs.push(input);
        self
    }

    /// `count` outputs, each polymorphic. [`output_types`](Self::output_types)
    /// when a test reads the declared type back.
    pub(crate) fn outputs(mut self, count: u32) -> Self {
        self.outputs.extend((0..count).map(|_| DataType::default()));
        self
    }

    pub(crate) fn output_types(mut self, types: impl IntoIterator<Item = DataType>) -> Self {
        self.outputs.extend(types);
        self
    }

    /// Place the node and hand back where it landed.
    pub(crate) fn add(self) -> Placed {
        let NodeBuilder {
            owner,
            mut e_node,
            inputs,
            outputs,
            events,
            node_id,
        } = self;
        let position = owner.placed.len();
        // The convention every hand-built fixture used, now in one place: ids
        // ascend from 1 with the placement, so `node_ids` stays sorted (which
        // `CompiledGraph::node` binary-searches) and index 0 is not the nil id.
        let node_id = node_id.unwrap_or_else(|| NodeId::from_u128(position as u128 + 1));
        if e_node.func_id.is_nil() {
            // Keyed to the *node's* id, not its position: one implementation per
            // node (the authoring default), and stable across a reinstall that
            // shifts the index space — where a func id that moved with the
            // position would read as a changed implementation and drop the
            // node's state.
            e_node.func_id = FuncId::from_u128(node_id.as_u128());
        }
        e_node.inputs = owner.program.inputs.append(inputs);
        e_node.outputs = owner.program.outputs.append(outputs);
        e_node.events = owner
            .program
            .events
            .append((0..events).map(|_| ExecutionEvent {
                subscribers: Vec::new(),
                lambda: EventLambda::default(),
            }));
        let node_idx = owner.program.push(node_id, e_node);
        let placed = Placed { node_id, node_idx };
        owner.placed.push(placed);
        placed
    }
}

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
    fn new(owner: &'a ProgramBuilder) -> Self {
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
