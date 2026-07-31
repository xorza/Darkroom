//! One run's derived state: the [`RunSchedule`] every phase between compile and
//! execute writes, and the two passes that fill it.
//!
//! [`Planner`](planner::Planner) opens it with one backward post-order DFS from the run's
//! roots (sinks, event subscribers, event-trigger owners — plus every sink when a fired
//! event reaches a [`RunSinks`](crate::graph::node::special::SpecialNode::RunSinks) sink),
//! producing `process_order` (deps before consumers) and each node's [`NodeState`]
//! (runnable, disabled, or blocked on inputs) — purely structural, no cache/digest
//! state. [`Scheduled::resolve`] then refines that same column against the
//! cache and fills the schedule's per-output half, and the executor reads both.
//!
//! Two passes rather than two pieces of state: this is the split a build system draws
//! between its dependency graph and its dirty / up-to-date analysis (Ninja/Bazel), or a
//! compiler between the CFG and a liveness pass — the schedule is structural, the
//! liveness cache-dependent, and every run redoes both. So the refinement happens in the
//! columns the planner opened rather than in a second buffer shadowing them.
//!
//! **The phase is in the type.** One buffer written by two passes needs the passes to
//! happen in order, and *that* the buffer cannot say: a `RunSchedule` looks the same
//! planned, resolved, or left over from last run. So neither pass hands back the buffer.
//! [`plan`](planner::Planner::plan) issues a [`Scheduled`], [`resolve`](Scheduled::resolve)
//! consumes it and issues a [`Resolved`], and the executor takes only that:
//!
//! ```text
//! &mut RunSchedule ──plan──▶ Scheduled ──resolve──▶ Resolved ──▶ Executor::run
//! ```
//!
//! Each handle carries the `CompiledGraph` its columns are `NodeIdx`-aligned to, so the pair
//! travels as one value instead of two arguments that could disagree. Executing an
//! unresolved plan, resolving one twice, and resolving against a different program than
//! you execute against all stop being mistakes the sequencing has to avoid and start
//! being programs that do not compile.
//!
//! Every method that reads or writes the schedule lives on it, here; the DFS scratch
//! the structural pass walks with is the one thing that isn't part of the answer, so it
//! is its own type in [`planner`]. The schedule is reused via a buffer on the engine and
//! the planner keeps its scratch across runs, so a repeated plan on an unchanged graph
//! allocates nothing — the handles borrow that buffer, they do not own a copy of it.

use crate::execution::schedule::error::RunScheduleValidationError;
use ::common::is_debug;

use crate::common::column::{Column, Idx};
use crate::common::set::IdxSet;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::compiled::{CompiledGraph, ExecutionBinding, ExecutionInput};
use crate::execution::error::{Error, Result};
use crate::execution::identity::{NodeIdx, OutputIdx};
use crate::execution::seeds::RunSeeds;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::node::special::SpecialNode;

pub(crate) mod error;
pub(crate) mod planner;

/// What becomes of one node this run — one column, written by two passes.
///
/// The planner establishes the structural three: `Disabled`, `MissingInputs`,
/// and `Cut`, its positive verdict. The cache-aware sweep
/// ([`Scheduled::resolve`])
/// then refines only the runnable ones, promoting what a running consumer
/// reads to `Run`, `Reuse`, or `MissingLambda` and leaving the rest where the
/// planner put them. That is why "the planner cleared it" and "the cut pruned
/// it" are one state rather than two: a node that could run and that nothing
/// this run reads *is* a cut node, and holding the two apart meant a column
/// each, kept in step by hand.
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
    /// Reached, but its func has no implementation. Report the error without probing its cache
    /// or keeping its input cone alive.
    MissingLambda,
    /// The node must run and owns one pending read for each bound input.
    Run,
}

impl NodeState {
    /// Whether the planner found this node structurally able to run. True of
    /// every state the sweep refines it into, so it answers the same before
    /// and after the sweep — which is what lets one column serve both.
    ///
    /// Every caller reads a node the walk settled: a `process_order` member, a
    /// root, or a `Bind` producer of one. `Unvisited` is the same broken
    /// schedule the exhaustive matches on this enum panic outright on — but
    /// those arms cost nothing, having to answer for the state either way,
    /// while this predicate has a total answer already and runs per node and
    /// per edge. So it asserts in debug and keeps the conservative `false`.
    pub(crate) fn is_runnable(self) -> bool {
        debug_assert_ne!(
            self,
            NodeState::Unvisited,
            "only a settled node is asked whether it runs"
        );
        !matches!(
            self,
            NodeState::Unvisited | NodeState::Disabled | NodeState::MissingInputs
        )
    }

    pub(crate) fn missing_required_inputs(self) -> bool {
        self == NodeState::MissingInputs
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
/// in place, and `outputs` is the same verdict counted per port. Holding them apart
/// let a freshly planned schedule carry the previous run's demand, or an executor read
/// counts resolved against a different one — states the single
/// [`reset_for_program`](Self::reset_for_program) rules out.
///
/// The buffer alone still cannot say *which* pass has run over it, so nothing outside
/// this module is handed one: a run reaches it through [`Scheduled`] and [`Resolved`],
/// which can only be minted in phase order.
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
    /// The nodes the backward walk started from — sinks, event subscribers,
    /// event-trigger owners, and node seeds — ascending. The schedule's "must be
    /// available" set: the sweep seeds liveness from these and prunes any cone
    /// reachable only through cache-hit consumers (see [`Scheduled::resolve`]).
    ///
    /// A list, because both readers walk it and neither asks whether a given
    /// node is in it; what a root *is* is the column below.
    ///
    /// Private, unlike the columns above: the two root fields are one fact in
    /// two records, and [`add_root`](Self::add_root) is what keeps them the same
    /// fact. Reached from outside by [`roots`](Self::roots) and
    /// [`root_flags`](Self::root_flags), which cannot write.
    roots: Vec<NodeIdx>,
    /// What each node is to this run's roots, aligned to the program's dense
    /// node vector.
    ///
    /// One column rather than a set per property, because the properties are
    /// not independent: a seeded node and an event source are *kinds of root*,
    /// and holding them in sets of their own made "seeded ⊆ roots" a pair of
    /// invariants to validate. Every bit is set through
    /// [`add_root`](Self::add_root) from a [`RootFlags`] constant that already
    /// carries [`RootFlags::PLAIN`], so a property off a root is not a state
    /// this can reach — crate-wide, which the three `pub(crate)` sets this
    /// replaced could not manage, any of them being insertable on its own.
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
    pub(crate) const SEEDED: Self = Self(Self::PLAIN.0 | 1 << 1);
    /// An event-owning root that must execute successfully to initialize the
    /// shared state its event lambdas consume. Unlike ordinary roots, these
    /// bypass cache reuse for the event-loop bootstrap run.
    pub(crate) const EVENT_SOURCE: Self = Self(Self::PLAIN.0 | 1 << 2);

    pub(crate) fn is_root(self) -> bool {
        self.0 & Self::PLAIN.0 != 0
    }

    pub(crate) fn is_seeded(self) -> bool {
        self.0 & Self::SEEDED.0 == Self::SEEDED.0
    }

    pub(crate) fn is_event_source(self) -> bool {
        self.0 & Self::EVENT_SOURCE.0 == Self::EVENT_SOURCE.0
    }

    /// Both sets of properties. A node reached twice by
    /// [`collect_roots`](RunSchedule::collect_roots) — a sink that also owns a
    /// subscribed event — keeps what each pass gave it.
    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl RunSchedule {
    /// Whether one input is unsatisfied: an unbound *required* port, or a bind to a
    /// producer that itself can't run (missing propagates only through non-runnable
    /// producers — a cached or executing one delivers a value, optional or not).
    /// This schedule must already hold the producer's state, which the planner's
    /// post-order forward pass guarantees. Shared by that pass and the executor's
    /// outcome so the two can't drift — and it reads the same in both, since every
    /// state the sweep writes is one that delivers a value.
    pub(crate) fn input_missing(&self, input: &ExecutionInput) -> bool {
        match &input.binding {
            ExecutionBinding::None => input.required,
            ExecutionBinding::Const(_) => false,
            ExecutionBinding::Bind(addr) => match self.states[addr.node_idx] {
                // The walk pushes every `Bind` producer before settling its
                // consumer, so an unvisited producer here means the schedule is
                // broken — not an input to answer for. Free to state: the arm
                // exists either way.
                NodeState::Unvisited => {
                    unreachable!("post-order settles a producer before its consumer is verdicted")
                }
                NodeState::Disabled => input.required,
                NodeState::MissingInputs => true,
                NodeState::Cut | NodeState::Reuse | NodeState::MissingLambda | NodeState::Run => {
                    false
                }
            },
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
        self.roots.clear();
        self.root_flags
            .reset(program.e_nodes.len(), RootFlags::default());
        self.outputs.reset(program.outputs.len());
    }

    /// The nodes the backward walk starts from, ascending.
    pub(crate) fn roots(&self) -> &[NodeIdx] {
        &self.roots
    }

    /// What one node is to this run's roots.
    pub(crate) fn root_flags(&self, node_idx: NodeIdx) -> RootFlags {
        self.root_flags[node_idx]
    }

    /// Record `node_idx` as a walk root carrying `flags`.
    ///
    /// The one way a root is added, so the list and the column cannot disagree:
    /// the push happens exactly once per node, on the call that first marks it,
    /// and a second call over the same node only unions in what it brings.
    pub(crate) fn add_root(&mut self, node_idx: NodeIdx, flags: RootFlags) {
        debug_assert!(flags.is_root(), "every root flag carries the root bit");
        if !self.root_flags[node_idx].is_root() {
            self.roots.push(node_idx);
        }
        self.root_flags[node_idx] = self.root_flags[node_idx].union(flags);
    }

    /// Collect the run's walk roots into `self.roots` — the seeds for both the backward
    /// walk and the executor's cut: exact execution-node seeds, every
    /// event subscriber, every sink node, and (for the event loop) every node owning a
    /// subscribed event.
    ///
    /// A [`RunSinks`](SpecialNode::RunSinks) node among a fired event's subscribers is not
    /// itself a root (it computes nothing); instead it promotes the run to include *every* sink
    /// node — the "when this event fires, re-run the whole graph" trigger.
    fn collect_roots(&mut self, program: &CompiledGraph, seeds: &RunSeeds) -> Result<()> {
        // `reset_for_program` already emptied the list and the column; this only adds.

        // Node seeds ("run to this node"): each exact execution node is a root, seeded so
        // every output is computed and its `disabled` flag is overridden for this run. An
        // id absent from the installed program is inconsistent caller state.
        for &node_id in &seeds.node_ids {
            let Some(&node_idx) = program.node_index.get(&node_id) else {
                return Err(Error::NodeSeedNotFound { node_id });
            };
            self.add_root(node_idx, RootFlags::SEEDED);
        }

        // Event subscribers. A `RunSinks` sink among them fires no cone of its own — it
        // promotes this run to run all sinks (below), so it's skipped as a root here.
        let mut run_sinks = seeds.sinks;
        for &event in &seeds.events {
            let Some(&owner_idx) = program.node_index.get(&event.node_id) else {
                return Err(Error::EventSeedNotFound { event });
            };
            let Some(e_event) = program.events[program[owner_idx].events].get(event.event_idx)
            else {
                return Err(Error::EventSeedNotFound { event });
            };
            let subs = &e_event.subscribers;
            for &sub_idx in subs {
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
        // Ascending, which is the order the previous bitset yielded and so the
        // order `process_order` was built in — a root list in the order the seeds
        // happened to name would make an unchanged graph plan differently run to
        // run. Deduplicated already, by `add_root` pushing once per node.
        self.roots.sort_unstable();
        Ok(())
    }

    /// A planned schedule is a unique post-order DFS whose bindings name valid outputs;
    /// disabled dependencies may remain outside the order.
    pub(crate) fn validate(
        &self,
        program: &CompiledGraph,
    ) -> std::result::Result<(), RunScheduleValidationError> {
        if self.process_order.len() > program.e_nodes.len() {
            return Err(RunScheduleValidationError::OrderTooLong);
        }

        // Establish that every column and set spans the program before the
        // index reads below rely on it — a validator must report the corruption
        // it finds, never fault on it. The output columns span the flat output
        // pool rather than the node vector, hence the per-entry expectation.
        for (set, len, expected) in [
            ("states", self.states.len(), program.e_nodes.len()),
            ("root flags", self.root_flags.len(), program.e_nodes.len()),
            (
                "output demand",
                self.outputs.demand.len(),
                program.outputs.len(),
            ),
            (
                "output readers",
                self.outputs.readers.len(),
                program.outputs.len(),
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
                if let ExecutionBinding::Bind(addr) = &input.binding {
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

        // A root the sweep would index the state and flag columns with. Nothing
        // else about the roots is checked, and nothing else needs to be: the list
        // and the column are written only by [`add_root`], which pushes once per
        // node and unions the flags, so they cannot disagree — and `RootFlags`
        // cannot spell a property without the root bit, which is what the two
        // "seeded/event source is not a root" checks here used to establish.
        for &node_idx in &self.roots {
            if node_idx.idx() >= program.e_nodes.len() {
                return Err(RunScheduleValidationError::NodeOutOfRange { node_idx });
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

/// A schedule the planner has filled, paired with the program it was planned against —
/// the only thing [`resolve`](Self::resolve) accepts, and the only thing `plan` hands
/// back.
///
/// The pair is the point. `NodeIdx` columns mean nothing without the program they are
/// aligned to, and a schedule means nothing before the sweep has run over it; holding
/// the two apart let a caller resolve against one program and execute against another,
/// or hand the executor a plan nothing had resolved — sequencing the engine got right
/// by hand and the types did not police at all.
#[derive(Debug)]
pub(crate) struct Scheduled<'a> {
    program: &'a CompiledGraph,
    schedule: &'a mut RunSchedule,
}

/// One resolved run: dispositions, demand, and reader counts derived together, over the
/// program they were derived from. Only [`Scheduled::resolve`] mints one, so holding it
/// *is* the proof that this schedule was planned and swept against this program.
///
/// `Copy` because that proof is the whole value: both halves are shared borrows, so a copy
/// vouches for exactly what the original did. The run loop takes one to walk the schedule,
/// and the engine keeps one to name the same pair when it closes the run out.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Resolved<'a> {
    program: &'a CompiledGraph,
    schedule: &'a RunSchedule,
}

impl<'a> Scheduled<'a> {
    /// Issued by [`plan`](planner::Planner::plan) once it has filled `schedule` for
    /// `program`.
    fn new(program: &'a CompiledGraph, schedule: &'a mut RunSchedule) -> Self {
        Scheduled { program, schedule }
    }

    /// The scheduled nodes that will actually run — what the filesystem prefetch walks
    /// between the two passes.
    pub(crate) fn executing(&self) -> impl Iterator<Item = NodeIdx> + '_ {
        self.schedule.executing()
    }

    /// Stamp the planned schedule, then sweep it in reverse for exact liveness and
    /// cache reuse, writing the result into that same buffer: the [`NodeState`] column
    /// the planner opened, plus the `outputs` half beside it. The cache-aware
    /// "up-to-date check" between [`plan`](planner::Planner::plan) and
    /// [`execute`](crate::execution::executor) — the planner's answer is *what could
    /// run*, this one is *what will*.
    ///
    /// Takes the schedule **by value**, so the pass cannot run twice over one plan
    /// (which would count every reader again) and the only way back to a `Scheduled`
    /// is to plan afresh.
    ///
    /// **Mutates `cache`** only to stamp each runnable node's `current_digest`: a live
    /// disk-cache frontier is *probed* from its blob header here and decoded later, by the
    /// run loop, when it reaches the node.
    ///
    /// In the sweep, a running consumer marks exactly the producer ports it reads; reuse,
    /// missing-input nodes, and missing lambdas stop the walk. Producer classification
    /// happens only after every downstream consumer has contributed, so cache coverage is
    /// checked against exact demand rather than the planner's structural
    /// over-approximation.
    ///
    /// Every node's digest is structural (a fold of its inputs and the run's prepared
    /// filesystem snapshot), so this stamps the *whole* graph ahead of the run. The one
    /// stamp it can leave imprecise is a digest folding a Bind-delivered path value it
    /// can't read yet: that folds to `None` here — "uncacheable, must run", which keeps
    /// the node's cone alive — and the run loop prepares the identity and re-stamps at
    /// reach time once its producers have settled, possibly improving `Run` to a reuse.
    pub(crate) async fn resolve(self, cache: &mut RuntimeCache) -> Resolved<'a> {
        let Scheduled { program, schedule } = self;
        // The cache holds no program of its own, so every question below names
        // the one this schedule was planned against — the handle's whole point.
        cache.stamp_digests(program, schedule.executing());
        // The sweep *accumulates* demand and readers, so it starts from zero of
        // its own accord rather than trusting whoever opened the schedule.
        schedule.outputs.reset(program.outputs.len());

        // Destructured so the sweep can read the schedule and the seed sets
        // while writing the state and output columns — disjoint fields of the
        // one buffer.
        let RunSchedule {
            process_order,
            states,
            roots,
            root_flags,
            outputs,
        } = &mut *schedule;

        for &node_idx in roots.iter() {
            // Only a root the planner cleared. Promoting a `Disabled` or
            // `MissingInputs` root would overwrite the verdict the run's
            // outcome reports it by — and claim a node the schedule may not
            // even contain.
            if states[node_idx].is_runnable() {
                states[node_idx] = NodeState::Run;
            }
        }

        for &node_idx in process_order.iter().rev() {
            // `Run` is written only through an `is_runnable` gate — here and
            // at the producer promotion below — so reaching this body already
            // means the planner cleared the node. There is no second check.
            if states[node_idx] != NodeState::Run {
                continue;
            }
            let e_node = &program[node_idx];
            if e_node.lambda.is_none() {
                states[node_idx] = NodeState::MissingLambda;
                continue;
            }
            let output_range = e_node.outputs;
            // A node seed ("run to this node") demands every output the node has:
            // the host asked for the node itself, not for what a consumer reads.
            if root_flags[node_idx].is_seeded() {
                outputs.demand[output_range].fill(OutputDemand::Produce);
            }
            let demand = &outputs.demand[output_range];
            if !root_flags[node_idx].is_event_source()
                && cache.probe_reuse(program, node_idx, demand).await
            {
                states[node_idx] = NodeState::Reuse;
                continue;
            }
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    // Only a producer the plan will actually run can deliver
                    // a value. A **disabled** producer feeding an *optional*
                    // input leaves the consumer perfectly schedulable —
                    // `input_missing` treats an optional port fed by a
                    // disabled producer as satisfied — but the producer
                    // itself never enters `process_order`. Marking it live
                    // here put a node the schedule does not contain into the
                    // run, and the consumer's read then demanded an output
                    // nothing would ever produce: a panic on a cold cache,
                    // and on a warm one the value from before it was
                    // disabled, served as if it were this run's.
                    //
                    // Reverse order also means a producer is promoted before
                    // it is classified, so this only ever overwrites the
                    // planner's `Cut` or an earlier consumer's `Run` — never
                    // a `Reuse` this sweep already settled.
                    if !states[addr.node_idx].is_runnable() {
                        continue;
                    }
                    states[addr.node_idx] = NodeState::Run;
                    outputs.add_reader(program.output_idx(*addr));
                }
            }
        }

        Resolved { program, schedule }
    }
}

impl<'a> Resolved<'a> {
    pub(crate) fn program(&self) -> &'a CompiledGraph {
        self.program
    }

    pub(crate) fn schedule(&self) -> &'a RunSchedule {
        self.schedule
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// Root inspection and the one re-rooting a fixture needs. Production writes
    /// roots only through [`collect_roots`](RunSchedule::collect_roots) and reads
    /// them only as a list plus per-node flags, so these exist for the tests that
    /// assert on one property at a time.
    impl RunSchedule {
        /// Drop every root, so a fixture built by another helper can name its own.
        pub(crate) fn clear_roots(&mut self) {
            self.roots.clear();
            self.root_flags
                .reset(self.states.len(), RootFlags::default());
        }

        /// The node-seeded roots, ascending — the old `seeded` set spelled out.
        pub(crate) fn seeded_roots(&self) -> Vec<NodeIdx> {
            self.roots_where(RootFlags::is_seeded)
        }

        /// The event-owning roots, ascending — the old `event_sources` set.
        pub(crate) fn event_source_roots(&self) -> Vec<NodeIdx> {
            self.roots_where(RootFlags::is_event_source)
        }

        fn roots_where(&self, property: fn(RootFlags) -> bool) -> Vec<NodeIdx> {
            self.roots
                .iter()
                .copied()
                .filter(|&node_idx| property(self.root_flags[node_idx]))
                .collect()
        }

        /// The root-flag column, for the validation test that shrinks it out from
        /// under the program it is supposed to span.
        pub(crate) fn root_flags_mut(&mut self) -> &mut Column<NodeIdx, RootFlags> {
            &mut self.root_flags
        }
    }

    /// The one way to mint a phase handle without the pass that would normally issue
    /// it, for fixtures that hand-build the columns a pass would have written — a
    /// sweep test starting from a schedule no planner produced, a run-loop test
    /// starting from dispositions no sweep produced.
    ///
    /// Test-only on purpose: the pairing each handle stands for is *asserted* here
    /// rather than established, which is exactly what production must never do.
    impl<'a> Scheduled<'a> {
        pub(crate) fn assume(program: &'a CompiledGraph, schedule: &'a mut RunSchedule) -> Self {
            Scheduled { program, schedule }
        }
    }

    impl<'a> Resolved<'a> {
        pub(crate) fn assume(program: &'a CompiledGraph, schedule: &'a RunSchedule) -> Self {
            Resolved { program, schedule }
        }
    }
}

#[cfg(test)]
mod tests;
