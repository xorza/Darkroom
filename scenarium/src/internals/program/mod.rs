//! [`ProgramBuilder`]: a [`CompiledGraph`] built by hand, for the tests below
//! the compiler.
//!
//! The schedule and cache tests want "a source, a consumer reading its port 0, a sink" and no
//! library, so they hand-build the artifact. The id↔index pair is a value ([`Placed`]), and a
//! binding is asked of the node it points at ([`Placed::out`]).
//!
//! The compiler's own tests use the real [`Compiler`](crate::Compiler); this is for programs a
//! compile *wouldn't* produce, such as a corrupt artifact a validator must reject.
//!
//! [`NodeBuilder`](node_builder::NodeBuilder) accumulates one node, [`Sweep`](sweep::Sweep) is
//! the cache-aware pass over a stated schedule (what it *became*), and [`Runs`](runs::Runs) is
//! the executor over one (what walking it *did*).

pub(crate) mod node_builder;
pub(crate) mod runs;
pub(crate) mod sweep;

use crate::data::const_value::ConstValue;
use crate::data::dynamic_value::DynamicValue;
use crate::exec::compile::compiled_graph::{CompiledGraph, ExecBinding};
use crate::exec::error::ExecResult;
use crate::exec::idx::{NodeIdx, OutputAddr};
use crate::exec::schedule::planner::Planner;
use crate::exec::schedule::{NodeState, RootFlags, RunSchedule};
use crate::exec::seeds::RunSeeds;
use crate::graph::identity::NodeId;
use crate::internals::program::node_builder::NodeBuilder;
use crate::internals::program::runs::Runs;
use crate::internals::program::sweep::Sweep;

/// Where one node landed: the stable id a host names it by, and the dense index every per-run
/// column is keyed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) node_id: NodeId,
    pub(crate) node_idx: NodeIdx,
}

impl Placed {
    /// A binding reading this node's output `port`.
    pub(crate) fn out(self, port: usize) -> ExecBinding {
        ExecBinding::Wire(self.addr(port))
    }

    /// This node's output `port` as the interned address a binding carries.
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
    /// The integer value a fixture's nodes produce and its caches hold.
    pub(crate) fn value(value: i64) -> DynamicValue {
        DynamicValue::Static(ConstValue::Int(value))
    }

    /// Open a node. Nothing lands until [`NodeBuilder::add`].
    pub(crate) fn node(&mut self) -> NodeBuilder<'_> {
        NodeBuilder::new(self)
    }

    pub(crate) fn program(&self) -> &CompiledGraph {
        &self.program
    }

    /// The finished artifact.
    pub(crate) fn into_program(self) -> CompiledGraph {
        self.program
    }

    pub(crate) fn program_mut(&mut self) -> &mut CompiledGraph {
        &mut self.program
    }

    /// Plan `seeds` over the program, surfacing the planner's refusal (a dependency cycle, or a
    /// seed the program does not hold).
    pub(crate) fn try_plan(&self, seeds: &RunSeeds) -> ExecResult<RunSchedule> {
        let mut schedule = RunSchedule::default();
        Planner::default().plan(&self.program, seeds, &mut schedule)?;
        Ok(schedule)
    }

    /// [`try_plan`](Self::try_plan), panicking on refusal.
    pub(crate) fn plan(&self, seeds: &RunSeeds) -> RunSchedule {
        self.try_plan(seeds).expect("the fixture program plans")
    }

    /// Plan every sink.
    pub(crate) fn plan_sinks(&self) -> RunSchedule {
        self.plan(&RunSeeds::sinks())
    }

    /// Stand up the cache-aware sweep over a schedule the fixture states directly (roots,
    /// blocked nodes), rather than one the planner produced.
    pub(crate) fn sweep(&self) -> Sweep<'_> {
        Sweep::new(self)
    }

    /// Stand up the executor over this program.
    pub(crate) fn runs(&self) -> Runs<'_> {
        Runs::new(self)
    }

    /// The planner's output, stated rather than provoked: every placed node in `process_order`,
    /// each verdicted `initial`, and no roots yet. The one place the "nodes run in placement
    /// order" convention lives.
    fn staged(&self, initial: NodeState) -> RunSchedule {
        let mut schedule = RunSchedule::default();
        schedule.reset_for_program(&self.program);
        schedule
            .process_order
            .extend(self.placed.iter().map(|placed| placed.node_idx));
        schedule.states.reset(self.program.e_nodes.len(), initial);
        schedule
    }

    /// [`staged`](Self::staged) at `initial` with every node a plain root.
    fn rooted(&self, initial: NodeState) -> RunSchedule {
        let mut schedule = self.staged(initial);
        for placed in &self.placed {
            schedule.add_root(placed.node_idx, RootFlags::PLAIN);
        }
        schedule
    }

    /// [`rooted`](Self::rooted) at the planner's positive verdict: the structural plan a
    /// whole-program run starts from.
    pub(crate) fn planned(&self) -> RunSchedule {
        self.rooted(NodeState::Cut)
    }

    /// [`rooted`](Self::rooted) with every node cleared to run.
    pub(super) fn running(&self) -> RunSchedule {
        self.rooted(NodeState::Run)
    }
}
