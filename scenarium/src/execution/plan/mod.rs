//! Scheduling: the per-run schedule ([`ExecutionPlan`]) and the [`Planner`] that builds
//! it. The planner runs one backward post-order DFS from the run's roots (sinks,
//! event subscribers, event-trigger owners — plus every sink when a fired event
//! reaches a [`RunSinks`](crate::node::special::SpecialNode::RunSinks) sink),
//! producing `process_order` (deps before consumers) and each node's [`NodeVerdict`]
//! (runnable, disabled, or blocked on inputs) — purely structural, no cache/digest
//! state. The resolver and executor consume the plan; it is reused via a buffer on the
//! engine and the `Planner` owns reusable DFS scratch, so a repeated plan on an
//! unchanged graph allocates nothing.

use crate::execution::compile::CompiledGraph;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::program::{ExecutionBinding, ExecutionInput, ExecutionProgram};
use crate::execution::{Error, NodeMap, NodeSet, Result, RunSeeds};
use crate::node::special::SpecialNode;

/// The planner's structural verdict for one node this run.
/// The planner decides whether it is runnable, disabled, or blocked on inputs;
/// *cached vs recompute* is a resolver call after planning. The default
/// (`MissingInputs`) is the conservative "not yet established as runnable" value;
/// disabled dependencies outside `process_order` receive the explicit
/// [`Disabled`](NodeVerdict::Disabled) verdict instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NodeVerdict {
    /// Runnable this round; the resolver then selects reuse or execution.
    Execute,
    /// Disabled for this run. Consumers treat it like an unbound input:
    /// required inputs fail while optional inputs remain runnable.
    Disabled,
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
/// outcome so the two can't drift.
pub(crate) fn input_missing(input: &ExecutionInput, verdicts: &NodeMap<NodeVerdict>) -> bool {
    match &input.binding {
        ExecutionBinding::None => input.required,
        ExecutionBinding::Const(_) => false,
        ExecutionBinding::Bind(addr) => match verdicts[&addr.e_node_id] {
            NodeVerdict::Execute => false,
            NodeVerdict::Disabled => input.required,
            NodeVerdict::MissingInputs => true,
        },
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionPlan {
    /// The schedule: post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the roots. Disabled dependencies stay outside the order unless they
    /// are explicit node seeds. The resolver refines it into the surviving run before
    /// execution.
    pub(crate) process_order: Vec<ExecutionNodeId>,
    /// Per-node verdict (execute / disabled / missing-inputs), keyed by node id.
    pub(crate) verdicts: NodeMap<NodeVerdict>,
    /// The nodes the backward walk started from — sinks, event subscribers,
    /// event-trigger owners, and node seeds. The schedule's "must be available" set:
    /// the resolver seeds liveness from these and prunes any cone reachable only through
    /// cache-hit consumers (see [`Resolver`](crate::execution::resolve::Resolver)).
    pub(crate) roots: NodeSet,
    /// The node-seeded roots (on-demand preview targets) — a *pinned root*, a subset of
    /// `roots`. Distinct from a pinned *output port* (a graph-authored, persisted flag —
    /// see [`Graph::pinned_outputs`](crate::graph::Graph)): this is a per-run seed with
    /// no persisted counterpart. Every output is demanded from the lambda and delivered
    /// to the host, while the node's cache mode remains the sole RAM-retention policy.
    pub(crate) pinned: NodeSet,
    /// Event-owning roots that must execute successfully to initialize the shared
    /// state their event lambdas consume. Unlike ordinary roots, these bypass cache
    /// reuse for the event-loop bootstrap run.
    pub(crate) event_sources: NodeSet,
}

impl ExecutionPlan {
    pub(crate) fn reset_for_program(&mut self, program: &ExecutionProgram) {
        self.process_order.clear();
        self.verdicts.clear();
        self.verdicts.extend(
            program
                .e_nodes
                .keys()
                .copied()
                .map(|e_node_id| (e_node_id, NodeVerdict::default())),
        );
        self.roots.clear();
        self.pinned.clear();
        self.event_sources.clear();
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
    Discover(ExecutionNodeId),
    Done(ExecutionNodeId),
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
    fn reset_for_program(&mut self, program: &ExecutionProgram) {
        self.stack.clear();
        self.color.clear();
        self.color.extend(
            program
                .e_nodes
                .keys()
                .copied()
                .map(|e_node_id| (e_node_id, Color::White)),
        );
    }

    /// Build the per-run schedule into `plan` from the compiled artifact and the run's
    /// `seeds` (the roots to walk back from). Exact execution-node seeds are roots
    /// directly. Errors on a dependency cycle or a node/event seed absent from the program.
    pub(crate) fn plan(
        &mut self,
        compiled: &CompiledGraph,
        seeds: &RunSeeds,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        let program = &compiled.program;
        plan.reset_for_program(program);
        self.reset_for_program(program);

        // Collect the walk roots straight into `plan.roots` — they seed the backward walk
        // below and the resolver's cache-aware reverse sweep.
        collect_roots(compiled, seeds, plan)?;

        let result = self.walk_backward_collect_order(program, plan);
        if result.is_ok() {
            plan.validate_debug(program);
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
        for e_node_id in plan.roots.iter().copied() {
            self.stack.push(Visit::Discover(e_node_id));
        }

        while let Some(visit) = self.stack.pop() {
            let e_node_id = match visit {
                Visit::Discover(e_node_id) => e_node_id,
                Visit::Done(e_node_id) => {
                    debug_assert_eq!(self.color[&e_node_id], Color::Gray);
                    *self.color.get_mut(&e_node_id).unwrap() = Color::Black;
                    plan.process_order.push(e_node_id);
                    // Runnable unless a required input is unbound or fed by a
                    // non-runnable producer. Post-order ⇒ deps already verdicted, so
                    // `input_missing` reads settled values. Whether the node's output is
                    // reused from cache is decided at execution, not here.
                    let missing = program
                        .node_inputs(&program.e_nodes[&e_node_id])
                        .iter()
                        .any(|e_input| input_missing(e_input, &plan.verdicts));
                    *plan.verdicts.get_mut(&e_node_id).unwrap() = if missing {
                        NodeVerdict::MissingInputs
                    } else {
                        NodeVerdict::Execute
                    };
                    continue;
                }
            };

            match self.color[&e_node_id] {
                Color::Gray => {
                    return Err(Error::CycleDetected { e_node_id });
                }
                Color::Black => continue,
                Color::White => {}
            }

            let e_node = &program.e_nodes[&e_node_id];
            // Disabled nodes block dependency traversal, but an explicit node
            // seed is pinned before this walk and overrides disable for this run.
            if e_node.disabled && !plan.pinned.contains(&e_node_id) {
                *self.color.get_mut(&e_node_id).unwrap() = Color::Black;
                *plan.verdicts.get_mut(&e_node_id).unwrap() = NodeVerdict::Disabled;
                continue;
            }

            *self.color.get_mut(&e_node_id).unwrap() = Color::Gray;
            self.stack.push(Visit::Done(e_node_id));

            for e_input in program.node_inputs(e_node) {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    self.stack.push(Visit::Discover(addr.e_node_id));
                }
            }
        }

        Ok(())
    }
}

/// Collect the run's walk roots into `plan.roots` — the seeds for both the backward walk and
/// the executor's cut: exact execution-node seeds, every
/// event subscriber, every sink node, and (for the event loop) every node owning a
/// subscribed event.
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

    // Node seeds (on-demand preview): each exact execution node is a root and pinned so
    // every output is computed and delivered. `pinned` also records the one-run disabled
    // override. An id absent from the installed program is inconsistent caller state.
    for &e_node_id in &seeds.nodes {
        if !program.e_nodes.contains_key(&e_node_id) {
            return Err(Error::NodeSeedNotFound { e_node_id });
        }
        plan.roots.insert(e_node_id);
        plan.pinned.insert(e_node_id);
    }

    // Event subscribers. A `RunSinks` sink among them fires no cone of its own — it
    // promotes this run to run all sinks (below), so it's skipped as a root here.
    let mut run_sinks = seeds.sinks;
    for &event in &seeds.events {
        let Some(e_node) = program.e_nodes.get(&event.e_node_id) else {
            return Err(Error::EventSeedNotFound { event });
        };
        let Some(e_event) = program.events[e_node.events.range()].get(event.event_idx) else {
            return Err(Error::EventSeedNotFound { event });
        };
        let subs = &e_event.subscribers;
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
    for e_node_id in program.e_nodes.keys().copied() {
        let e_node = &program.e_nodes[&e_node_id];
        if e_node.disabled {
            continue;
        }
        if run_sinks && e_node.sink {
            plan.roots.insert(e_node_id);
        }
        if seeds.event_triggers
            && program.events[e_node.events.range()]
                .iter()
                .any(|event| !event.subscribers.is_empty())
        {
            plan.roots.insert(e_node_id);
            plan.event_sources.insert(e_node_id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
