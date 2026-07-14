//! Scheduling: the per-run schedule ([`ExecutionPlan`]) and the [`Planner`] that builds
//! it. The planner runs one backward post-order DFS from the run's roots (sinks,
//! event subscribers, event-trigger owners — plus every sink when a fired event
//! reaches a [`RunSinks`](crate::node::special::SpecialNode::RunSinks) sink),
//! producing `process_order` (deps before consumers), output demand + reader counts, and each
//! node's [`NodeVerdict`] (runnable vs blocked on inputs) — purely structural, no
//! cache/digest state. The
//! [`Executor`](crate::execution::executor::Executor) consumes the plan; the plan is
//! reused via a buffer on the engine and the `Planner` owns reusable DFS scratch, so a
//! repeated plan on an unchanged graph allocates nothing.

use crate::execution::compile::CompiledGraph;
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionProgram, NodeIdx, OutputIdx,
};
use crate::execution::query::resolve_node_idx;
use crate::execution::{Error, NodeColumn, OutputColumn, Result, RunSeeds, validate};
use crate::node::lambda::OutputDemand;
use crate::node::special::SpecialNode;

/// The planner's structural verdict for one node this run, indexed by `e_node_idx`.
/// The planner decides only *runnable vs blocked on inputs*; *cached vs recompute* is an
/// execution-time call (the executor computes the node's digest and reuses from RAM/disk
/// or runs). The default (`MissingInputs`) is the conservative "not yet established as
/// runnable" value for nodes outside `process_order`, whose verdict is never read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NodeVerdict {
    /// Runnable this round (the executor then reuses its cached output or recomputes it).
    Execute,
    /// A required input is unsatisfied (unbound, or fed by a non-runnable producer);
    /// can't run, and the "missing" verdict propagates to its consumers.
    #[default]
    MissingInputs,
}

impl NodeVerdict {
    pub(crate) fn wants_execute(self) -> bool {
        self == NodeVerdict::Execute
    }
    pub(crate) fn missing_required_inputs(self) -> bool {
        self == NodeVerdict::MissingInputs
    }
}

/// Whether one input is unsatisfied: an unbound *required* port, or a bind to a
/// producer that itself can't run (missing propagates only through non-runnable
/// producers — a cached or executing one delivers a value, optional or not).
/// `verdicts` must already hold the producer's verdict, which the planner's
/// post-order forward pass guarantees. Shared by that pass and the executor's
/// stats so the two can't drift.
pub(crate) fn input_missing(input: &ExecutionInput, verdicts: &NodeColumn<NodeVerdict>) -> bool {
    match &input.binding {
        ExecutionBinding::None => input.required,
        ExecutionBinding::Const(_) => false,
        ExecutionBinding::Bind(addr) => verdicts[addr.target_idx].missing_required_inputs(),
    }
}

#[derive(Debug, Default)]
pub(crate) struct PlannedOutputs {
    /// Whether each output must be produced for an in-graph reader or a host pin.
    pub(crate) demand: OutputColumn<OutputDemand>,
    /// Structural downstream binding count. Pins do not create readers.
    pub(crate) readers: OutputColumn<u32>,
}

impl PlannedOutputs {
    fn reset(&mut self, output_count: usize) {
        self.demand.reset(output_count, OutputDemand::Skip);
        self.readers.reset(output_count, 0);
    }

    fn seed_external_demand(&mut self, program: &ExecutionProgram, pinned: &[NodeIdx]) {
        assert_eq!(
            program.output_pinned.len(),
            program.n_outputs(),
            "output_pinned must have exactly one entry per pooled output port"
        );
        for output_idx in program.pinned_output_indices() {
            self.demand[output_idx] = OutputDemand::Produce;
        }
        for &idx in pinned {
            self.demand
                .slice_mut(program.e_nodes[idx].outputs)
                .fill(OutputDemand::Produce);
        }
    }

    fn add_reader(&mut self, output_idx: OutputIdx) {
        self.readers[output_idx] = self.readers[output_idx]
            .checked_add(1)
            .expect("output reader count overflowed u32");
        self.demand[output_idx] = OutputDemand::Produce;
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionPlan {
    /// The schedule: post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the sinks — every reachable node, producer-first. The executor
    /// walks this and skips `MissingInputs` nodes (and reuses cached ones) inline.
    pub(crate) process_order: Vec<NodeIdx>,
    /// Per-node verdict (execute / missing-inputs), indexed by node position.
    pub(crate) verdicts: NodeColumn<NodeVerdict>,
    /// Per-output production demand and structural binding-reader counts. The plan owns
    /// both immutable templates; the executor copies only `readers` into live state.
    pub(crate) outputs: PlannedOutputs,
    /// The nodes the backward walk started from — sinks, event subscribers,
    /// event-trigger owners, and node seeds (a node seeding via several categories may
    /// repeat; harmless). The schedule's "must be available" set: the executor's pre-run
    /// cut seeds its `needed` mask from these and prunes any cone reachable only through
    /// cache-hit consumers (see [`Executor`](crate::execution::executor::Executor)).
    pub(crate) roots: Vec<NodeIdx>,
    /// The node-seeded roots (on-demand preview targets) — a *pinned root*, a subset of
    /// `roots`. Distinct from a pinned *output port* (a graph-authored, persisted flag —
    /// see [`Graph::pinned_outputs`](crate::graph::Graph)): this is a per-run seed with
    /// no persisted counterpart. Drives two things: every output is demanded from the
    /// lambda, and the executor's per-run retention policy (`Executor::retain`) keeps the node's
    /// outputs resident through every release/eviction site whatever its cache mode.
    /// Retention is all it takes for a repeated run to be a RAM hit: the reuse check
    /// serves any resident digest-valid value.
    pub(crate) pinned: Vec<NodeIdx>,
}

impl ExecutionPlan {
    pub(crate) fn clear(&mut self) {
        self.process_order.clear();
        self.verdicts.values.clear();
        self.outputs.demand.values.clear();
        self.outputs.readers.values.clear();
        self.roots.clear();
        self.pinned.clear();
    }

    /// Clear the order and reset every per-node verdict to default at the given pool
    /// sizes. Called at the start of each planning pass.
    pub(crate) fn reset(&mut self, n_nodes: usize, n_outputs: usize) {
        self.process_order.clear();
        self.verdicts.reset(n_nodes, NodeVerdict::default());
        self.outputs.reset(n_outputs);
        self.roots.clear();
        self.pinned.clear();
    }
}

/// DFS coloring for the backward pass. White = unvisited, Gray = on
/// stack (Done pushed, children pending), Black = children done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

#[derive(Debug)]
enum Visit {
    Discover(NodeIdx),
    Done(NodeIdx),
}

/// Reusable per-run scheduling scratch, kept across runs so a repeated plan on
/// an unchanged graph does no scheduling allocations.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    /// DFS coloring for the backward pass.
    color: NodeColumn<Color>,
    /// DFS work stack.
    stack: Vec<Visit>,
}

impl Planner {
    /// Build the per-run schedule into `plan` from the compiled artifact and the run's
    /// `seeds` (the roots to walk back from); the artifact's flatten map resolves node
    /// seeds (authoring ids) to flat roots. Errors on a dependency cycle or an
    /// unresolvable node seed.
    pub(crate) fn plan(
        &mut self,
        compiled: &CompiledGraph,
        seeds: &RunSeeds,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        let program = &compiled.program;
        plan.reset(program.e_nodes.len(), program.n_outputs());

        // Collect the walk roots straight into `plan.roots` — they seed the backward walk
        // below *and* the executor's pre-run cut, so they live on the plan as an output.
        // Must run before external demand is seeded because pinned roots are collected here.
        collect_roots(compiled, seeds, plan)?;

        plan.outputs.seed_external_demand(program, &plan.pinned);

        let result = self.walk_backward_collect_order(program, plan);
        if result.is_ok() {
            validate::schedule(program, plan);
        }
        result
    }

    /// Backward post-order DFS from the roots: builds `process_order` (deps before
    /// consumers), records output demand and readers, detects cycles, and — folded in here
    /// rather than a separate forward pass — resolves each node's [`NodeVerdict`].
    /// The verdict is set in the `Done` arm, i.e. in post-order, so every Bind dep is
    /// already `Black` with its own verdict set when a consumer reads it (what the old
    /// separate `resolve_verdicts` pass asserted, now structural).
    fn walk_backward_collect_order(
        &mut self,
        program: &ExecutionProgram,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        // `plan.reset` (called at the top of `plan`) already cleared `process_order` and
        // `roots`; this pass only needs to reset its own scratch.
        self.stack.clear();
        self.color.reset(program.e_nodes.len(), Color::White);

        for e_node_idx in plan.roots.iter().copied() {
            self.stack.push(Visit::Discover(e_node_idx));
        }

        while let Some(visit) = self.stack.pop() {
            let idx = match visit {
                Visit::Discover(idx) => idx,
                Visit::Done(idx) => {
                    assert_eq!(self.color[idx], Color::Gray);
                    self.color[idx] = Color::Black;
                    plan.process_order.push(idx);
                    // Runnable unless a required input is unbound or fed by a
                    // non-runnable producer. Post-order ⇒ deps already verdicted, so
                    // `input_missing` reads settled values. Whether the node's output is
                    // reused from cache is decided at execution, not here.
                    let inputs = program.e_nodes[idx].inputs;
                    let missing = program.inputs[inputs.range()]
                        .iter()
                        .any(|e_input| input_missing(e_input, &plan.verdicts));
                    plan.verdicts[idx] = if missing {
                        NodeVerdict::MissingInputs
                    } else {
                        NodeVerdict::Execute
                    };
                    continue;
                }
            };

            match self.color[idx] {
                Color::Gray => {
                    return Err(Error::CycleDetected {
                        node_id: program.e_nodes[idx].id,
                    });
                }
                Color::Black => continue,
                Color::White => {}
            }

            self.color[idx] = Color::Gray;
            self.stack.push(Visit::Done(idx));

            let span = program.e_nodes[idx].inputs;
            for e_input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    let output_idx = program.output_idx(addr.target_idx, addr.port_idx);
                    plan.outputs.add_reader(output_idx);
                    self.stack.push(Visit::Discover(addr.target_idx));
                }
            }
        }

        Ok(())
    }
}

/// Collect the run's walk roots into `plan.roots` — the seeds for both the backward walk and
/// the executor's cut: the node seeds (authoring ids resolved to flat nodes here), every
/// event subscriber, every sink node, and (for the event loop) every node owning a
/// subscribed event. Not deduped: a node seeding via several categories appears more than
/// once, which is harmless — the walk's `Color` check skips a revisited root and the cut's
/// `needed[root] = true` seeding is idempotent, so neither cares about repeats.
///
/// A [`RunSinks`](SpecialNode::RunSinks) node among a fired event's subscribers is not
/// itself a root (it computes nothing); instead it promotes the run to include *every* sink
/// node — the "when this event fires, re-run the whole graph" trigger.
fn collect_roots(
    compiled: &CompiledGraph,
    seeds: &RunSeeds,
    plan: &mut ExecutionPlan,
) -> Result<()> {
    let program = &compiled.program;
    // `plan.reset` already cleared `roots`/`pinned`; this only pushes into them.

    // Node seeds (on-demand preview): roots like any other, plus pinned so their outputs
    // are computed and retained (see `ExecutionPlan::pinned`). Seeds are batched with the
    // program they target, so an id that doesn't resolve (deleted, disabled, or stale) is
    // inconsistent caller state — fail the run rather than silently skip the seed.
    for address in &seeds.nodes {
        let idx = resolve_node_idx(compiled, address).ok_or_else(|| Error::NodeSeedNotFound {
            address: address.clone(),
        })?;
        plan.roots.push(idx);
        plan.pinned.push(idx);
    }

    // Event subscribers. A `RunSinks` sink among them fires no cone of its own — it
    // promotes this run to run all sinks (below), so it's skipped as a root here.
    let mut run_sinks = seeds.sinks;
    for event in &seeds.events {
        let e_node = program.e_nodes.by_key(&event.node_id).unwrap();
        let subs = &program.events[e_node.events.range()][event.event_idx].subscribers;
        for &sub in subs {
            if program.e_nodes[sub].special == Some(SpecialNode::RunSinks) {
                run_sinks = true;
            } else {
                plan.roots.push(sub);
            }
        }
    }

    if !run_sinks && !seeds.event_triggers {
        return Ok(());
    }
    // One sweep for both whole-graph seed kinds: sink nodes (requested directly, or
    // promoted by a fired event reaching a `RunSinks` sink) and — for the event
    // loop — nodes owning a subscribed event.
    for (idx, e_node) in program.e_nodes.iter().enumerate() {
        if (run_sinks && e_node.sink)
            || (seeds.event_triggers
                && program.events[e_node.events.range()]
                    .iter()
                    .any(|ev| !ev.subscribers.is_empty()))
        {
            plan.roots.push(idx.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
