//! Scheduling: the per-run schedule ([`ExecutionPlan`]) and the [`Planner`] that builds
//! it. The planner runs one backward post-order DFS from the run's roots (sinks,
//! event subscribers, event-trigger owners — plus every sink when a fired event
//! reaches a [`RunSinks`](crate::node::special::SpecialNode::RunSinks) sink),
//! producing `process_order` (deps before consumers) and each node's [`NodeState`]
//! (runnable, disabled, or blocked on inputs) — purely structural, no cache/digest
//! state. The resolver refines that same column in place and the executor reads it;
//! the plan is reused via a buffer on the engine and the `Planner` owns reusable DFS
//! scratch, so a repeated plan on an unchanged graph allocates nothing.

use crate::execution::error::{Error, Result};
use crate::execution::program::index::{NodeColumn, NodeIdx, NodeSet};
use crate::execution::program::{ExecutionBinding, ExecutionInput, Program};
use crate::execution::seeds::RunSeeds;
use crate::node::special::SpecialNode;

/// What becomes of one node this run — one column, written by two passes.
///
/// The planner establishes the structural three: `Disabled`, `MissingInputs`,
/// and `Cut`, its positive verdict. The [`Resolver`](crate::execution::resolve::Resolver)
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
    /// Disabled for this run. Consumers treat it like an unbound input:
    /// required inputs fail while optional inputs remain runnable.
    Disabled,
    /// A required input is unsatisfied (unbound, or fed by a non-runnable producer);
    /// can't run, and the "missing" verdict propagates to its consumers. The
    /// default: the conservative "not yet established as runnable" value.
    #[default]
    MissingInputs,
    /// Runnable, and read by nothing that runs — pruned by the cut. Both the
    /// planner's "this node can run" and the resolver's "and no one needs it",
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
    /// every state the resolver refines it into, so it answers the same before
    /// and after the sweep — which is what lets one column serve both.
    pub(crate) fn is_runnable(self) -> bool {
        !matches!(self, NodeState::Disabled | NodeState::MissingInputs)
    }

    pub(crate) fn missing_required_inputs(self) -> bool {
        self == NodeState::MissingInputs
    }
}

/// Whether one input is unsatisfied: an unbound *required* port, or a bind to a
/// producer that itself can't run (missing propagates only through non-runnable
/// producers — a cached or executing one delivers a value, optional or not).
/// `states` must already hold the producer's state, which the planner's
/// post-order forward pass guarantees. Shared by that pass and the executor's
/// outcome so the two can't drift — and it reads the same in both, since every
/// state the resolver writes is one that delivers a value.
pub(crate) fn input_missing(input: &ExecutionInput, states: &NodeColumn<NodeState>) -> bool {
    match &input.binding {
        ExecutionBinding::None => input.required,
        ExecutionBinding::Const(_) => false,
        ExecutionBinding::Bind(addr) => match states[addr.node_idx] {
            NodeState::Disabled => input.required,
            NodeState::MissingInputs => true,
            NodeState::Cut | NodeState::Reuse | NodeState::MissingLambda | NodeState::Run => false,
        },
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionPlan {
    /// The schedule: post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the roots. Disabled dependencies stay outside the order unless they
    /// are explicit node seeds. The resolver refines it into the surviving run before
    /// execution.
    pub(crate) process_order: Vec<NodeIdx>,
    /// Per-node [`NodeState`], aligned to the program's dense node vector. The
    /// planner writes the structural verdict; the resolver refines it in place.
    pub(crate) states: NodeColumn<NodeState>,
    /// The nodes the backward walk started from — sinks, event subscribers,
    /// event-trigger owners, and node seeds. The schedule's "must be available" set:
    /// the resolver seeds liveness from these and prunes any cone reachable only through
    /// cache-hit consumers (see [`Resolver`](crate::execution::resolve::Resolver)).
    pub(crate) roots: NodeSet,
    /// The node-seeded roots ("run to this node") — a subset of `roots`, carrying a
    /// per-run seed with
    /// no persisted counterpart. Every output is demanded from the lambda and delivered
    /// to the host, while the node's cache mode remains the sole RAM-retention policy.
    pub(crate) seeded: NodeSet,
    /// Event-owning roots that must execute successfully to initialize the shared
    /// state their event lambdas consume. Unlike ordinary roots, these bypass cache
    /// reuse for the event-loop bootstrap run.
    pub(crate) event_sources: NodeSet,
}

impl ExecutionPlan {
    /// The scheduled nodes that will actually run, producer-first — the
    /// schedule minus the disabled and input-blocked ones. What the digest
    /// pass and the filesystem prefetch each walk, so "runnable this run" is
    /// stated once rather than re-derived by every consumer of the plan.
    pub(crate) fn executing(&self) -> impl Iterator<Item = NodeIdx> + '_ {
        self.process_order
            .iter()
            .copied()
            .filter(|&node_idx| self.states[node_idx].is_runnable())
    }

    pub(crate) fn reset_for_program(&mut self, program: &Program) {
        self.process_order.clear();
        self.states
            .reset(program.e_nodes.len(), NodeState::default());
        self.roots.reset(program.e_nodes.len());
        self.seeded.reset(program.e_nodes.len());
        self.event_sources.reset(program.e_nodes.len());
    }
}

/// DFS coloring for the backward pass. White = unvisited, Gray = on
/// stack (Done pushed, children pending), Black = children done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Color {
    #[default]
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
    fn reset_for_program(&mut self, program: &Program) {
        self.stack.clear();
        self.color.reset(program.e_nodes.len(), Color::White);
    }

    /// Build the per-run schedule into `plan` from the installed program and the run's
    /// `seeds` (the roots to walk back from). Exact execution-node seeds are roots
    /// directly. Errors on a dependency cycle or a node/event seed absent from the program.
    pub(crate) fn plan(
        &mut self,
        program: &Program,
        seeds: &RunSeeds,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.reset_for_program(program);
        self.reset_for_program(program);

        // Collect the walk roots straight into `plan.roots` — they seed the backward walk
        // below and the resolver's cache-aware reverse sweep.
        collect_roots(program, seeds, plan)?;

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
        program: &Program,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        for node_idx in plan.roots.iter() {
            self.stack.push(Visit::Discover(node_idx));
        }

        while let Some(visit) = self.stack.pop() {
            let node_idx = match visit {
                Visit::Discover(node_idx) => node_idx,
                Visit::Done(node_idx) => {
                    debug_assert_eq!(self.color[node_idx], Color::Gray);
                    self.color[node_idx] = Color::Black;
                    plan.process_order.push(node_idx);
                    // Runnable unless a required input is unbound or fed by a
                    // non-runnable producer. Post-order ⇒ deps already verdicted, so
                    // `input_missing` reads settled values. Whether the node's output is
                    // reused from cache is decided at execution, not here.
                    let missing = program.inputs[program[node_idx].inputs]
                        .iter()
                        .any(|e_input| input_missing(e_input, &plan.states));
                    // `Cut` is the planner's *positive* verdict — runnable, and
                    // nothing has claimed it yet. The resolver's sweep promotes
                    // the ones a running consumer reads and leaves the rest here.
                    plan.states[node_idx] = if missing {
                        NodeState::MissingInputs
                    } else {
                        NodeState::Cut
                    };
                    continue;
                }
            };

            match self.color[node_idx] {
                Color::Gray => {
                    return Err(Error::CycleDetected {
                        e_node_id: program.e_node_ids[node_idx],
                    });
                }
                Color::Black => continue,
                Color::White => {}
            }

            let e_node = &program[node_idx];
            // Disabled nodes block dependency traversal, but an explicit node
            // seed is recorded before this walk and overrides disable for this run.
            if e_node.disabled && !plan.seeded.contains(node_idx) {
                self.color[node_idx] = Color::Black;
                plan.states[node_idx] = NodeState::Disabled;
                continue;
            }

            self.color[node_idx] = Color::Gray;
            self.stack.push(Visit::Done(node_idx));

            for e_input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    self.stack.push(Visit::Discover(addr.node_idx));
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
fn collect_roots(program: &Program, seeds: &RunSeeds, plan: &mut ExecutionPlan) -> Result<()> {
    // `plan.reset` already cleared `roots`/`seeded`; this only pushes into them.

    // Node seeds ("run to this node"): each exact execution node is a root and seeded so
    // every output is computed. `seeded` also records the one-run disabled
    // override. An id absent from the installed program is inconsistent caller state.
    for &e_node_id in &seeds.e_node_ids {
        let Some(&node_idx) = program.e_node_index.get(&e_node_id) else {
            return Err(Error::NodeSeedNotFound { e_node_id });
        };
        plan.roots.insert(node_idx);
        plan.seeded.insert(node_idx);
    }

    // Event subscribers. A `RunSinks` sink among them fires no cone of its own — it
    // promotes this run to run all sinks (below), so it's skipped as a root here.
    let mut run_sinks = seeds.sinks;
    for &event in &seeds.events {
        let Some(&owner_idx) = program.e_node_index.get(&event.e_node_id) else {
            return Err(Error::EventSeedNotFound { event });
        };
        let Some(e_event) = program.events[program[owner_idx].events].get(event.event_idx) else {
            return Err(Error::EventSeedNotFound { event });
        };
        let subs = &e_event.subscribers;
        for &sub_idx in subs {
            if program[sub_idx].special == Some(SpecialNode::RunSinks) {
                run_sinks = true;
            } else {
                plan.roots.insert(sub_idx);
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
            plan.roots.insert(node_idx);
        }
        if seeds.event_sources
            && program.events[e_node.events]
                .iter()
                .any(|event| !event.subscribers.is_empty())
        {
            plan.roots.insert(node_idx);
            plan.event_sources.insert(node_idx);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
