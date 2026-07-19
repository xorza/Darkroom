//! Scheduling: the per-run schedule ([`ExecutionPlan`]) and the [`Planner`] that builds
//! it. The planner runs one backward post-order DFS from the run's roots (sinks,
//! event subscribers, event-trigger owners — plus every sink when a fired event
//! reaches a [`RunSinks`](crate::node::special::SpecialNode::RunSinks) sink),
//! producing `process_order` (deps before consumers) and each node's [`NodeVerdict`]
//! (runnable vs blocked on inputs) — purely structural, no cache/digest state. The
//! resolver and executor consume the plan; it is reused via a buffer on the engine and
//! the `Planner` owns reusable DFS scratch, so a repeated plan on an unchanged graph
//! allocates nothing.

use crate::execution::compile::CompiledGraph;
use crate::execution::program::{ExecutionBinding, ExecutionInput, ExecutionProgram};
use crate::execution::query::resolve_node_id;
use crate::execution::{Error, NodeMap, NodeSet, Result, RunSeeds, reset_node_map, validate};
use crate::graph::NodeId;
use crate::node::special::SpecialNode;

/// The planner's structural verdict for one node this run.
/// The planner decides only *runnable vs blocked on inputs*; *cached vs recompute* is an
/// resolver call after planning. The default (`MissingInputs`) is the conservative "not
/// yet established as runnable" value for nodes outside `process_order`, whose verdict
/// is never read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NodeVerdict {
    /// Runnable this round; the resolver then selects reuse or execution.
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
pub(crate) fn input_missing(input: &ExecutionInput, verdicts: &NodeMap<NodeVerdict>) -> bool {
    match &input.binding {
        ExecutionBinding::None => input.required,
        ExecutionBinding::Const(_) => false,
        ExecutionBinding::Bind(addr) => verdicts[&addr.target].missing_required_inputs(),
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionPlan {
    /// The schedule: post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the sinks — every reachable node, producer-first. The resolver
    /// refines it into the surviving run before execution.
    pub(crate) process_order: Vec<NodeId>,
    /// Per-node verdict (execute / missing-inputs), keyed by node id.
    pub(crate) verdicts: NodeMap<NodeVerdict>,
    /// The nodes the backward walk started from — sinks, event subscribers,
    /// event-trigger owners, and node seeds. The schedule's "must be available" set:
    /// the resolver seeds liveness from these and prunes any cone reachable only through
    /// cache-hit consumers (see [`Resolver`](crate::execution::resolve::Resolver)).
    pub(crate) roots: NodeSet,
    /// The node-seeded roots (on-demand preview targets) — a *pinned root*, a subset of
    /// `roots`. Distinct from a pinned *output port* (a graph-authored, persisted flag —
    /// see [`Graph::pinned_outputs`](crate::graph::Graph)): this is a per-run seed with
    /// no persisted counterpart. Drives two things: every output is demanded from the
    /// lambda, and the executor's per-run retention policy (`Executor::retain`) keeps the node's
    /// outputs resident through every release/eviction site whatever its cache mode.
    /// Retention is all it takes for a repeated run to be a RAM hit: the reuse check
    /// serves any resident digest-valid value.
    pub(crate) pinned: NodeSet,
}

impl ExecutionPlan {
    pub(crate) fn clear(&mut self) {
        self.process_order.clear();
        self.verdicts.clear();
        self.roots.clear();
        self.pinned.clear();
    }

    /// Clear the order and reset every per-node verdict to default at the given pool
    /// sizes. Called at the start of each planning pass.
    pub(crate) fn reset(&mut self, program: &ExecutionProgram) {
        self.process_order.clear();
        reset_node_map(
            &mut self.verdicts,
            program.e_nodes.keys().copied(),
            NodeVerdict::default(),
        );
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
    Discover(NodeId),
    Done(NodeId),
}

/// Reusable per-run scheduling scratch, kept across runs so a repeated plan on
/// an unchanged graph does no scheduling allocations.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    /// DFS coloring for the backward pass.
    color: NodeMap<Color>,
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
        plan.reset(program);

        // Collect the walk roots straight into `plan.roots` — they seed the backward walk
        // below and the resolver's cache-aware reverse sweep.
        collect_roots(compiled, seeds, plan)?;

        let result = self.walk_backward_collect_order(program, plan);
        if result.is_ok() {
            validate::schedule(program, plan);
        }
        result
    }

    /// Backward post-order DFS from the roots: builds `process_order` (deps before
    /// consumers), detects cycles, and — folded in here rather than a separate forward
    /// pass — resolves each node's [`NodeVerdict`].
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
        reset_node_map(
            &mut self.color,
            program.e_nodes.keys().copied(),
            Color::White,
        );

        for node_id in plan.roots.iter().copied() {
            self.stack.push(Visit::Discover(node_id));
        }

        while let Some(visit) = self.stack.pop() {
            let node_id = match visit {
                Visit::Discover(node_id) => node_id,
                Visit::Done(node_id) => {
                    assert_eq!(self.color[&node_id], Color::Gray);
                    *self.color.get_mut(&node_id).unwrap() = Color::Black;
                    plan.process_order.push(node_id);
                    // Runnable unless a required input is unbound or fed by a
                    // non-runnable producer. Post-order ⇒ deps already verdicted, so
                    // `input_missing` reads settled values. Whether the node's output is
                    // reused from cache is decided at execution, not here.
                    let inputs = program.e_nodes[&node_id].inputs;
                    let missing = program.inputs[inputs.range()]
                        .iter()
                        .any(|e_input| input_missing(e_input, &plan.verdicts));
                    *plan.verdicts.get_mut(&node_id).unwrap() = if missing {
                        NodeVerdict::MissingInputs
                    } else {
                        NodeVerdict::Execute
                    };
                    continue;
                }
            };

            match self.color[&node_id] {
                Color::Gray => {
                    return Err(Error::CycleDetected { node_id });
                }
                Color::Black => continue,
                Color::White => {}
            }

            *self.color.get_mut(&node_id).unwrap() = Color::Gray;
            self.stack.push(Visit::Done(node_id));

            let span = program.e_nodes[&node_id].inputs;
            for e_input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    self.stack.push(Visit::Discover(addr.target));
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
/// once.
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
        let node_id =
            resolve_node_id(compiled, address).ok_or_else(|| Error::NodeSeedNotFound {
                address: address.clone(),
            })?;
        plan.roots.insert(node_id);
        plan.pinned.insert(node_id);
    }

    // Event subscribers. A `RunSinks` sink among them fires no cone of its own — it
    // promotes this run to run all sinks (below), so it's skipped as a root here.
    let mut run_sinks = seeds.sinks;
    for event in &seeds.events {
        let e_node = &program.e_nodes[&event.node_id];
        let subs = &program.events[e_node.events.range()][event.event_idx].subscribers;
        for &sub in subs {
            if program.e_nodes[&sub].special == Some(SpecialNode::RunSinks) {
                run_sinks = true;
            } else {
                plan.roots.insert(sub);
            }
        }
    }

    if !run_sinks && !seeds.event_triggers {
        return Ok(());
    }
    // One sweep for both whole-graph seed kinds: sink nodes (requested directly, or
    // promoted by a fired event reaching a `RunSinks` sink) and — for the event
    // loop — nodes owning a subscribed event.
    for node_id in program.e_nodes.keys().copied() {
        let e_node = &program.e_nodes[&node_id];
        if (run_sinks && e_node.sink)
            || (seeds.event_triggers
                && program.events[e_node.events.range()]
                    .iter()
                    .any(|ev| !ev.subscribers.is_empty()))
        {
            plan.roots.insert(node_id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
