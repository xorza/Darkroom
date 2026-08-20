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
//!
//! The three things a fixture drives the builder with each own a file:
//! [`NodeBuilder`](node_builder::NodeBuilder) accumulates one node,
//! [`Sweep`](sweep::Sweep) is the cache-aware pass over a stated schedule, and
//! [`Runs`](runs::Runs) is the executor over one.

pub(crate) mod node_builder;
pub(crate) mod runs;
pub(crate) mod sweep;

use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionBinding};
use crate::execution::error::Result;
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::execution::schedule::planner::Planner;
use crate::execution::schedule::{NodeState, RootFlags, RunSchedule};
use crate::execution::seeds::RunSeeds;
use crate::graph::identity::NodeId;
use crate::testing::program::node_builder::NodeBuilder;
use crate::testing::program::runs::Runs;
use crate::testing::program::sweep::Sweep;

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
        NodeBuilder::new(self)
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
        Sweep::new(self)
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
