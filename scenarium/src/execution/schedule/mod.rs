//! One run's derived state: the [`RunSchedule`] every phase between compile and
//! execute writes, and the two passes that fill it.
//!
//! [`Planner`](planner::Planner) opens it with one backward post-order DFS from the run's
//! roots (sinks, event subscribers, event-trigger owners — plus every sink when a fired
//! event reaches a [`RunSinks`](crate::graph::node::special::SpecialNode::RunSinks) sink),
//! producing `process_order` (deps before consumers) and each node's [`NodeState`]
//! (runnable, disabled, or blocked on inputs) — purely structural, no cache/digest
//! state. [`resolve::sweep`](crate::execution::resolve::sweep) then refines that same
//! column against the cache and fills the schedule's per-output half, and the executor
//! reads both. The sweep lives in its own module: it reads the cache, and this one does
//! not.
//!
//! Two passes rather than two pieces of state: this is the split a build system draws
//! between its dependency graph and its dirty / up-to-date analysis (Ninja/Bazel), or a
//! compiler between the CFG and a liveness pass — the schedule is structural, the
//! liveness cache-dependent, and every run redoes both. So the refinement happens in the
//! columns the planner opened rather than in a second buffer shadowing them.
//!
//! **The order is the caller's to keep.** One buffer written by two passes has to be
//! written in order, and the buffer itself cannot say which pass has run: a
//! `RunSchedule` looks the same planned, resolved, or left over from last run. The
//! sequence has one production call site —
//! [`ExecutionEngine::execute`](crate::execution::engine) — where the three steps are
//! consecutive statements:
//!
//! ```text
//! Planner::plan ──▶ resolve::sweep ──▶ Executor::run
//! ```
//!
//! Every step names the `CompiledGraph` its `NodeIdx` columns are aligned to, and
//! [`validate`](RunSchedule::validate) — asserted in debug after each pass — is what
//! catches a schedule spanning a different program than the one it is being read
//! against.
//!
//! Every method of the schedule lives on it, here — the cache-aware sweep is a function
//! over the schedule and the cache, not a method of either. The DFS scratch the
//! structural pass walks with is the one thing that isn't part of the answer, so it is
//! its own type in [`planner`]. The schedule is reused via a buffer on the engine and
//! the planner keeps its scratch across runs, so a repeated plan on an unchanged graph
//! allocates nothing.

use common::is_debug;

use crate::containers::column::Column;
use crate::containers::idx_set::IdxSet;
use crate::data::const_value::ConstValue;
use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionBinding, ExecutionInput};
use crate::execution::error::{ExecutionError, ExecutionResult};
use crate::execution::idx::{NodeIdx, OutputAddr, OutputIdx};
use crate::execution::schedule::error::{RunScheduleValidationError, ScheduleColumn};
use crate::execution::seeds::RunSeeds;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::node::special::SpecialNode;

pub(crate) mod error;
pub(crate) mod planner;

/// What becomes of one node this run — one column, written by two passes.
///
/// The planner establishes the structural three: `Disabled`, `MissingInputs`,
/// and `Cut`, its positive verdict. The cache-aware sweep
/// ([`resolve::sweep`](crate::execution::resolve::sweep)) then refines only the
/// runnable ones, promoting what a running consumer reads to `Run` or `Reuse` and
/// leaving the rest where the planner put them. That is why "the planner cleared
/// it" and "the cut pruned it" are one state rather than two: a node that could
/// run and that nothing this run reads *is* a cut node.
///
/// Authoritative once resolved: a `Reuse` is never re-derived after the cut may
/// have pruned its producers. The safe direction is allowed once — the run loop
/// re-stamps a `Run` whose bound path value was not readable in time and may
/// serve its cache after all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NodeState {
    /// The backward walk never reached this node, so no pass has decided
    /// anything about it — the fill [`reset_for_program`](RunSchedule::reset_for_program)
    /// leaves behind. A distinct state rather than a conservative real verdict,
    /// so "unreachable from any root" cannot be read as "visited and blocked":
    /// [`validate`](RunSchedule::validate) checks that exactly the scheduled
    /// nodes left it.
    #[default]
    Unvisited,
    /// Disabled for this run. Consumers treat it like an unbound input:
    /// required inputs fail while optional inputs remain runnable.
    Disabled,
    /// A required input is unsatisfied (unbound, or fed by a non-runnable producer);
    /// can't run, and the "missing" verdict propagates to its consumers.
    MissingInputs,
    /// Runnable, and read by nothing that runs — pruned by the cut. Both the
    /// planner's "this node can run" and the sweep's "and no one needs it",
    /// since a runnable node the sweep never promotes is exactly that.
    Cut,
    /// An unchanged demanded output is verified *available* — resident, or covered by a
    /// digest-matched blob the run loop decodes when it reaches the node. Serve it without
    /// running the lambda.
    Reuse,
    /// The node must run and owns one pending read for each bound input.
    Run,
}

impl NodeState {
    /// Whether the planner found this node structurally able to run. True of
    /// every state the sweep refines it into, so it answers the same before
    /// and after the sweep — which is what lets one column serve both.
    ///
    /// Every caller reads a node the walk settled: a `process_order` member, a
    /// root, or a `Wire` producer of one. `Unvisited` is the same broken
    /// schedule the exhaustive matches on this enum panic outright on — but
    /// those arms cost nothing, having to answer for the state either way,
    /// while this predicate has a total answer already and runs per node and
    /// per edge. So it asserts in debug and keeps the conservative `false`.
    pub(crate) fn is_runnable(self) -> bool {
        debug_assert_ne!(
            self,
            Self::Unvisited,
            "only a settled node is asked whether it runs"
        );
        !matches!(self, Self::Unvisited | Self::Disabled | Self::MissingInputs)
    }

    pub(crate) const fn missing_required_inputs(self) -> bool {
        matches!(self, Self::MissingInputs)
    }

    /// Clear this node to run, reporting whether it was this call that did it.
    ///
    /// The only way `Run` is written, and the gate is the point: promoting a
    /// `Disabled` or `MissingInputs` node would overwrite the verdict the run's
    /// outcome reports it by, and — for a disabled producer feeding an optional
    /// input — claim a node `process_order` does not even contain.
    pub(super) fn promote_to_run(&mut self) -> bool {
        let cleared = self.is_runnable();
        if cleared {
            *self = Self::Run;
        }
        cleared
    }
}

/// The per-output half of one resolved run, filled by the same reverse sweep that
/// refines the [`NodeState`] column beside it — so a cut, reused, or blocked consumer
/// contributes neither demand nor a reader to its producers.
#[derive(Debug, Default)]
pub(crate) struct ResolvedOutputs {
    /// Whether each output must be produced for a live reader or a host pin.
    pub(crate) demand: Column<OutputIdx, OutputDemand>,
    /// Consumers which will actually run and read each output.
    pub(crate) readers: Column<OutputIdx, u32>,
}

impl ResolvedOutputs {
    /// Clear both columns to "nothing demanded, nobody reading". Called from
    /// [`RunSchedule::reset_for_program`], so a planned schedule carries no
    /// previous run's counts, and again from the sweep that fills them, so
    /// counting always starts from zero rather than accumulating onto whatever
    /// was there.
    pub(crate) fn reset(&mut self, output_count: usize) {
        self.demand.reset(output_count, OutputDemand::Skip);
        self.readers.reset(output_count, 0);
    }

    pub(crate) fn add_reader(&mut self, output_idx: OutputIdx) {
        let readers = &mut self.readers[output_idx];
        debug_assert_ne!(*readers, u32::MAX, "output reader count overflowed u32");
        *readers = readers.wrapping_add(1);
        self.demand[output_idx] = OutputDemand::Produce;
    }
}

/// Everything one run derives from the installed program: the schedule, its per-node
/// verdicts, the seed sets the backward walk started from, and the per-output demand
/// and reader counts.
///
/// One buffer rather than a plan and a resolution held apart, because the two passes
/// that write it produce one answer: the sweep already refines the planner's `states`
/// in place, and `outputs` is the same verdict counted per port. Held apart, they would
/// let a freshly planned schedule carry the previous run's demand, or an executor read
/// counts resolved against a different one — states the single
/// [`reset_for_program`](Self::reset_for_program) rules out.
///
/// The buffer alone cannot say *which* pass has run over it; what stands in for that is
/// [`validate`](Self::validate), asserted in debug after each pass.
#[derive(Debug, Default)]
pub(crate) struct RunSchedule {
    /// The schedule: post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the roots. Disabled dependencies stay outside the order unless they
    /// are explicit node seeds. The sweep refines it into the surviving run before
    /// execution.
    pub(crate) process_order: Vec<NodeIdx>,
    /// Per-node [`NodeState`], aligned to the program's dense node vector. The
    /// planner writes the structural verdict; the sweep refines it in place.
    pub(crate) states: Column<NodeIdx, NodeState>,
    /// What each node is to this run's roots — the nodes the backward walk
    /// started from: sinks, event subscribers, event-trigger owners, and node
    /// seeds — aligned to the program's dense node vector. The schedule's "must
    /// be available" set: the sweep seeds liveness from these and prunes any
    /// cone reachable only through cache-hit consumers (see
    /// [`resolve::sweep`](crate::execution::resolve::sweep)).
    ///
    /// The only record of the roots: [`roots`](Self::roots) reads the list off
    /// it, so the list is ascending whatever order the seeds named them in.
    ///
    /// One column rather than a set per property, because the properties are
    /// not independent: a seeded node and an event source are *kinds of root*.
    /// Private, and every bit is set through [`add_root`](Self::add_root) from
    /// a [`RootFlags`] constant that already carries [`RootFlags::PLAIN`], so a
    /// property off a root is not a state this can reach — crate-wide.
    root_flags: Column<NodeIdx, RootFlags>,
    /// Exact per-output demand and live reader counts, written by the sweep
    /// once the state column above is settled.
    pub(crate) outputs: ResolvedOutputs,
}

/// What one node is to a run's roots: whether the backward walk started from it
/// at all, and which of the two properties a root can carry it has.
///
/// The named constants are the only way in, and each of them includes the root
/// bit — which is what makes "a seeded node is a root" true by construction
/// rather than by a check after the fact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RootFlags(u8);

impl RootFlags {
    /// A root with no property beyond being one — a sink, or an event subscriber.
    pub(crate) const PLAIN: Self = Self(1 << 0);
    /// A node-seeded root ("run to this node"), carrying a per-run seed with no
    /// persisted counterpart. Every output is demanded from the lambda and
    /// delivered to the host, and the seed overrides the node's `disabled` flag
    /// for this run, while its cache mode remains the sole RAM-retention policy.
    pub(crate) const SEEDED: Self = Self(Self::PLAIN.0 | (1 << 1));
    /// An event-owning root that must execute successfully to initialize the
    /// shared state its event lambdas consume. Unlike ordinary roots, these
    /// bypass cache reuse for the event-loop bootstrap run.
    pub(crate) const EVENT_SOURCE: Self = Self(Self::PLAIN.0 | (1 << 2));

    pub(crate) const fn is_root(self) -> bool {
        self.0 & Self::PLAIN.0 != 0
    }

    pub(crate) const fn is_seeded(self) -> bool {
        self.0 & Self::SEEDED.0 == Self::SEEDED.0
    }

    pub(crate) const fn is_event_source(self) -> bool {
        self.0 & Self::EVENT_SOURCE.0 == Self::EVENT_SOURCE.0
    }

    /// Both sets of properties. A node reached twice by
    /// [`collect_roots`](RunSchedule::collect_roots) — a sink that also owns a
    /// subscribed event — keeps what each pass gave it.
    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// What one input hands its node this run — the one rule the planner's
/// missing-input verdict, the digest fold, and the run loop's input read all
/// answer from, so the value a node is keyed by is the value it is handed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery<'a> {
    /// Nothing: no binding, or a bind to a producer disabled for this run.
    Unbound,
    /// Nothing, because the input that overrides this one delivers. Never
    /// missing, required or not: the node reads the overriding input instead.
    SetAside,
    /// Nothing, because the producer cannot run for want of an input of its
    /// own — which makes this input missing, optional or not. Only a node the
    /// planner verdicted `MissingInputs` has one, so a node that runs never
    /// reads it.
    Blocked,
    Const(&'a ConstValue),
    /// The producer port, whose producer runs or is served this run.
    Wire(OutputAddr),
}

impl Delivery<'_> {
    /// Whether the node is handed a value: a constant other than the explicit
    /// `Null`, or a producer's output.
    const fn delivers(self) -> bool {
        match self {
            Self::Const(value) => !matches!(value, ConstValue::Null),
            Self::Wire(_) => true,
            Self::Unbound | Self::SetAside | Self::Blocked => false,
        }
    }
}

impl RunSchedule {
    /// What `input` delivers this run. This schedule must already hold the
    /// state of `input`'s producer and of its overrider's producer, which the
    /// planner's post-order walk guarantees.
    pub(crate) fn delivery<'a>(
        &self,
        program: &'a CompiledGraph,
        input: &'a ExecutionInput,
    ) -> Delivery<'a> {
        // An overrider is never itself overridden (`FuncBuilder::validate`), so its
        // own binding is the whole answer.
        if input
            .overridden_by
            .is_some_and(|by| self.binding_delivery(&program.inputs[by]).delivers())
        {
            return Delivery::SetAside;
        }
        self.binding_delivery(input)
    }

    /// [`delivery`](Self::delivery) without the override.
    fn binding_delivery<'a>(&self, input: &'a ExecutionInput) -> Delivery<'a> {
        match &input.binding {
            ExecutionBinding::None => Delivery::Unbound,
            ExecutionBinding::Const(value) => Delivery::Const(value),
            ExecutionBinding::Wire(addr) => match self.states[addr.node_idx] {
                // The walk pushes every `Wire` producer before settling its
                // consumer, so an unvisited producer here means the schedule is
                // broken — not an input to answer for.
                NodeState::Unvisited => {
                    unreachable!("post-order settles a producer before its consumer is read")
                }
                NodeState::Disabled => Delivery::Unbound,
                NodeState::MissingInputs => Delivery::Blocked,
                NodeState::Cut | NodeState::Reuse | NodeState::Run => Delivery::Wire(*addr),
            },
        }
    }

    /// Whether one input is unsatisfied: an unbound *required* port, or a bind to a
    /// producer that itself can't run for a missing input (missing propagates only
    /// through those — a cached or executing producer delivers a value, and a
    /// disabled one leaves the input unbound). Shared by the planner's verdict and
    /// the executor's outcome so the two can't drift.
    pub(crate) fn input_missing(&self, program: &CompiledGraph, input: &ExecutionInput) -> bool {
        match self.delivery(program, input) {
            Delivery::Unbound => input.required,
            Delivery::Blocked => true,
            Delivery::SetAside | Delivery::Const(_) | Delivery::Wire(_) => false,
        }
    }

    /// The scheduled nodes that will actually run, producer-first — the
    /// schedule minus the disabled and input-blocked ones. What the digest
    /// pass and the filesystem prefetch each walk, so "runnable this run" is
    /// stated once rather than re-derived by every consumer of the schedule.
    pub(crate) fn executing(&self) -> impl Iterator<Item = NodeIdx> + '_ {
        self.process_order
            .iter()
            .copied()
            .filter(|&node_idx| self.states[node_idx].is_runnable())
    }

    /// Clear every column for a run over `program` — including the output half,
    /// so a schedule the sweep has not yet filled cannot be read as the previous
    /// run's demand.
    pub(crate) fn reset_for_program(&mut self, program: &CompiledGraph) {
        self.process_order.clear();
        self.states
            .reset(program.e_nodes.len(), NodeState::default());
        self.root_flags
            .reset(program.e_nodes.len(), RootFlags::default());
        self.outputs.reset(program.output_count as usize);
    }

    /// The nodes the backward walk starts from, ascending.
    pub(crate) fn roots(&self) -> impl Iterator<Item = NodeIdx> + '_ {
        self.root_flags
            .iter_indexed()
            .filter(|(_, flags)| flags.is_root())
            .map(|(node_idx, _)| node_idx)
    }

    /// What one node is to this run's roots.
    pub(crate) fn root_flags(&self, node_idx: NodeIdx) -> RootFlags {
        self.root_flags[node_idx]
    }

    /// Record `node_idx` as a walk root carrying `flags`. A second call over the
    /// same node only unions in what it brings.
    pub(crate) fn add_root(&mut self, node_idx: NodeIdx, flags: RootFlags) {
        debug_assert!(flags.is_root(), "every root flag carries the root bit");
        self.root_flags[node_idx] = self.root_flags[node_idx].union(flags);
    }

    /// Clear every root to run — where the sweep's liveness starts: what the run
    /// was asked for.
    pub(super) fn promote_roots(&mut self) {
        for (node_idx, flags) in self.root_flags.iter_indexed() {
            if flags.is_root() {
                self.states[node_idx].promote_to_run();
            }
        }
    }

    /// Collect the run's walk roots into `self.root_flags` — the seeds for both the backward
    /// walk and the executor's cut: exact execution-node seeds, every
    /// event subscriber, every sink node, and (for the event loop) every node owning a
    /// subscribed event.
    ///
    /// A [`RunSinks`](SpecialNode::RunSinks) node among a fired event's subscribers is not
    /// itself a root (it computes nothing); instead it promotes the run to include *every* sink
    /// node — the "when this event fires, re-run the whole graph" trigger.
    fn collect_roots(&mut self, program: &CompiledGraph, seeds: &RunSeeds) -> ExecutionResult<()> {
        // `reset_for_program` already emptied the column; this only adds.

        // Node seeds ("run to this node"): each exact execution node is a root, seeded so
        // every output is computed and its `disabled` flag is overridden for this run. An
        // id absent from the installed program is inconsistent caller state.
        for &node_id in &seeds.node_ids {
            let Some(node_idx) = program.node(node_id) else {
                return Err(ExecutionError::NodeSeedNotFound { node_id });
            };
            self.add_root(node_idx, RootFlags::SEEDED);
        }

        // Event subscribers. A `RunSinks` sink among them fires no cone of its own — it
        // promotes this run to run all sinks (below), so it's skipped as a root here.
        let mut run_sinks = seeds.sinks;
        // A fired event names the installed program: events arrive in a batch
        // of their own, and each install rebuilds the event loop.
        for &event in &seeds.events {
            let owner_idx = program
                .node(event.node_id)
                .expect("a fired event's emitter is in the installed program");
            let e_event = program.events[program[owner_idx].events]
                .get(event.event_idx)
                .expect("a fired event is one its emitter declares");
            for &sub_idx in &program.subscribers[e_event.subscribers] {
                if program[sub_idx].special == Some(SpecialNode::RunSinks) {
                    run_sinks = true;
                } else {
                    self.add_root(sub_idx, RootFlags::PLAIN);
                }
            }
        }

        if !run_sinks && !seeds.event_sources {
            return Ok(());
        }
        // One sweep for both whole-graph seed kinds: sink nodes (requested directly, or
        // promoted by a fired event reaching a `RunSinks` sink) and — for the event
        // loop — nodes owning a subscribed event.
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            if e_node.disabled {
                continue;
            }
            if run_sinks && e_node.sink {
                self.add_root(node_idx, RootFlags::PLAIN);
            }
            if seeds.event_sources
                && program.events[e_node.events]
                    .iter()
                    .any(|event| !event.subscribers.is_empty())
            {
                self.add_root(node_idx, RootFlags::EVENT_SOURCE);
            }
        }
        Ok(())
    }

    /// A planned schedule is a unique post-order DFS whose bindings name valid outputs;
    /// disabled dependencies may remain outside the order.
    pub(crate) fn validate(
        &self,
        program: &CompiledGraph,
    ) -> Result<(), RunScheduleValidationError> {
        if self.process_order.len() > program.e_nodes.len() {
            return Err(RunScheduleValidationError::OrderTooLong);
        }

        // Establish that every column and set spans the program before the
        // index reads below rely on it — a validator must report the corruption
        // it finds, never fault on it. The output columns span the flat output
        // pool rather than the node vector, hence the per-entry expectation.
        for (set, len, expected) in [
            (
                ScheduleColumn::States,
                self.states.len(),
                program.e_nodes.len(),
            ),
            (
                ScheduleColumn::RootFlags,
                self.root_flags.len(),
                program.e_nodes.len(),
            ),
            (
                ScheduleColumn::OutputDemand,
                self.outputs.demand.len(),
                program.output_count as usize,
            ),
            (
                ScheduleColumn::OutputReaders,
                self.outputs.readers.len(),
                program.output_count as usize,
            ),
        ] {
            if len != expected {
                return Err(RunScheduleValidationError::SetLength { set, len, expected });
            }
        }

        let mut seen_in_order = IdxSet::default();
        seen_in_order.reset(program.e_nodes.len());
        for &node_idx in &self.process_order {
            let e_node = program
                .e_nodes
                .get(node_idx)
                .ok_or(RunScheduleValidationError::NodeOutOfRange { node_idx })?;
            let node_id = program.node_ids[node_idx];
            let inputs = program
                .inputs
                .get_span(e_node.inputs)
                .ok_or(RunScheduleValidationError::InputRange { node_id })?;
            for input in inputs {
                if let ExecutionBinding::Wire(addr) = &input.binding {
                    // Resolve the dependency before probing the sets: an
                    // out-of-range target is the corruption to report, not a
                    // reason to index past `seen_in_order` and `node_ids`.
                    let dependency = program.e_nodes.get(addr.node_idx).ok_or(
                        RunScheduleValidationError::NodeOutOfRange {
                            node_idx: addr.node_idx,
                        },
                    )?;
                    let disabled_dependency =
                        dependency.disabled && self.states[addr.node_idx] == NodeState::Disabled;
                    if !seen_in_order.contains(addr.node_idx) && !disabled_dependency {
                        return Err(RunScheduleValidationError::BeforeDependency {
                            node_id,
                            dependency: program.node_ids[addr.node_idx],
                        });
                    }
                }
            }
            if seen_in_order.contains(node_idx) {
                return Err(RunScheduleValidationError::DuplicateNode { node_id });
            }
            seen_in_order.insert(node_idx);
        }

        // A state decided for a node the schedule left out. The other direction
        // — a scheduled node still holding the `Unvisited` fill — needs no check
        // here: every site that reads one panics on it outright.
        for (node_idx, _) in program.e_nodes.iter_indexed() {
            let state = self.states[node_idx];
            if !seen_in_order.contains(node_idx)
                && !matches!(state, NodeState::Unvisited | NodeState::Disabled)
            {
                return Err(RunScheduleValidationError::UnscheduledNodeDecided {
                    node_id: program.node_ids[node_idx],
                    state,
                });
            }
        }

        Ok(())
    }

    /// Debug-only assert form of [`Self::validate`].
    pub(crate) fn validate_debug(&self, program: &CompiledGraph) {
        if !is_debug() {
            return;
        }
        self.validate(program)
            .expect("run schedule invariant violated");
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// Root inspection. Production writes roots only through
    /// [`collect_roots`](RunSchedule::collect_roots) and reads them only as a
    /// walk plus per-node flags, so these exist for the tests that assert on
    /// one property at a time.
    impl RunSchedule {
        /// The node-seeded roots, ascending.
        pub(crate) fn seeded_roots(&self) -> Vec<NodeIdx> {
            self.roots_where(RootFlags::is_seeded)
        }

        /// The event-owning roots, ascending.
        pub(crate) fn event_source_roots(&self) -> Vec<NodeIdx> {
            self.roots_where(RootFlags::is_event_source)
        }

        fn roots_where(&self, property: fn(RootFlags) -> bool) -> Vec<NodeIdx> {
            self.roots()
                .filter(|&node_idx| property(self.root_flags[node_idx]))
                .collect()
        }

        /// The root-flag column, for the validation test that shrinks it out from
        /// under the program it is supposed to span.
        pub(crate) fn root_flags_mut(&mut self) -> &mut Column<NodeIdx, RootFlags> {
            &mut self.root_flags
        }
    }
}

#[cfg(test)]
mod tests;
