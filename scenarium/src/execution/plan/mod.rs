//! Scheduling: the per-run schedule ([`ExecutionPlan`]) and the [`Planner`] that builds
//! it. The planner runs one backward post-order DFS from the run's roots (terminals,
//! event subscribers, event-trigger owners — plus every terminal when a fired event
//! reaches a [`RunTerminals`](crate::node::special::SpecialNode::RunTerminals) sink),
//! producing `process_order` (deps before consumers), per-output usage counts, and each
//! node's [`NodeVerdict`] (runnable vs blocked on inputs) — purely structural, no
//! cache/digest state. The
//! [`Executor`](crate::execution::executor::Executor) consumes the plan; the plan is
//! reused via a buffer on the engine and the `Planner` owns reusable DFS scratch, so a
//! repeated plan on an unchanged graph allocates nothing.

use crate::execution::program::{ExecutionBinding, ExecutionInput, ExecutionProgram, NodeIdx};
use crate::execution::query::resolve_node_idx;
use crate::execution::stats::FlattenMap;
use crate::execution::{Error, NodeColumn, Result, RunSeeds, validate};
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
pub(crate) struct ExecutionPlan {
    /// The schedule: post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the terminals — every reachable node, producer-first. The executor
    /// walks this and skips `MissingInputs` nodes (and reuses cached ones) inline.
    pub(crate) process_order: Vec<NodeIdx>,
    /// Per-node verdict (execute / missing-inputs), indexed by node position.
    pub(crate) verdicts: NodeColumn<NodeVerdict>,
    /// Per-output consumer counts, indexed by output-pool index. `> 0` ⇒ the output
    /// is `Needed` this run; `0` ⇒ `Skip`. The executor passes the count through to
    /// each lambda as [`OutputUsage`](crate::node::func_lambda::OutputUsage) so a node can
    /// skip computing outputs nobody reads.
    pub(crate) output_usage: Vec<u32>,
    /// The nodes the backward walk started from — terminals, event subscribers,
    /// event-trigger owners, and node seeds (a node seeding via several categories may
    /// repeat; harmless). The schedule's "must be available" set: the executor's pre-run
    /// cut seeds its `needed` mask from these and prunes any cone reachable only through
    /// cache-hit consumers (see [`Executor`](crate::execution::executor::Executor)).
    pub(crate) roots: Vec<NodeIdx>,
    /// The node-seeded roots (on-demand preview targets), a subset of `roots`. Pinning
    /// feeds the executor's per-run retention policy (`Executor::retain`): the node's
    /// outputs stay resident through every release/eviction site whatever its cache
    /// mode, and an output with zero in-run consumers is still computed (its usage
    /// floors at `Needed(1)` — the preview fetch reads it after the run). Retention is
    /// all it takes for a repeated run to be a RAM hit: the reuse check serves any
    /// resident digest-valid value.
    pub(crate) pinned: Vec<NodeIdx>,
}

impl ExecutionPlan {
    pub(crate) fn clear(&mut self) {
        self.process_order.clear();
        self.verdicts.clear();
        self.output_usage.clear();
        self.roots.clear();
        self.pinned.clear();
    }

    /// Clear the order and reset every per-node verdict to default at the given pool
    /// sizes. Called at the start of each planning pass.
    pub(crate) fn reset(&mut self, n_nodes: usize, n_outputs: usize) {
        self.process_order.clear();
        self.verdicts.reset(n_nodes, NodeVerdict::default());
        self.output_usage.clear();
        self.output_usage.resize(n_outputs, 0);
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

/// Why a node sits on the DFS stack. `Discover` means "reach this node" — as a walk root
/// or as a producer reached from a consumer, handled identically. `Done` is the post-order
/// marker pushed under a node's children. Output-usage is counted at push time (per consumer
/// edge), so the discovery carries no port.
#[derive(Debug)]
enum VisitCause {
    Discover,
    Done,
}

#[derive(Debug)]
struct Visit {
    e_node_idx: NodeIdx,
    cause: VisitCause,
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
    /// Build the per-run schedule into `plan` from the program and the run's `seeds`
    /// (the roots to walk back from); `flatten` resolves node seeds (authoring ids)
    /// to flat roots. Errors only on a dependency cycle.
    pub(crate) fn plan(
        &mut self,
        program: &ExecutionProgram,
        flatten: &FlattenMap,
        seeds: &RunSeeds,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.reset(program.e_nodes.len(), program.n_outputs());

        // Collect the walk roots straight into `plan.roots` — they seed the backward walk
        // below *and* the executor's pre-run cut, so they live on the plan as an output.
        collect_roots(program, flatten, seeds, plan);

        let result = self.walk_backward_collect_order(program, plan);
        if result.is_ok() {
            validate::schedule(program, plan);
        }
        result
    }

    /// Backward post-order DFS from the roots: builds `process_order` (deps before
    /// consumers), counts per-output usage, detects cycles, and — folded in here
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
            self.stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Discover,
            });
        }

        while let Some(visit) = self.stack.pop() {
            match visit.cause {
                VisitCause::Discover => {}
                VisitCause::Done => {
                    let idx = visit.e_node_idx;
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
            }

            let idx = visit.e_node_idx;
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
            self.stack.push(Visit {
                e_node_idx: idx,
                cause: VisitCause::Done,
            });

            let span = program.e_nodes[idx].inputs;
            for e_input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    // Count this consumer's read of the producer's port (drives the
                    // executor's per-output Skip/Needed); once per consumer edge,
                    // counted at push so the visit cause needs no payload.
                    let outputs = program.e_nodes[addr.target_idx].outputs;
                    plan.output_usage[outputs.start as usize + addr.port_idx] += 1;
                    self.stack.push(Visit {
                        e_node_idx: addr.target_idx,
                        cause: VisitCause::Discover,
                    });
                }
            }
        }

        Ok(())
    }
}

/// Collect the run's walk roots into `plan.roots` — the seeds for both the backward walk and
/// the executor's cut: the node seeds (authoring ids resolved to flat nodes here), every
/// event subscriber, every terminal node, and (for the event loop) every node owning a
/// subscribed event. Not deduped: a node seeding via several categories appears more than
/// once, which is harmless — the walk's `Color` check skips a revisited root and the cut's
/// `needed[root] = true` seeding is idempotent, so neither cares about repeats.
///
/// A [`RunTerminals`](SpecialNode::RunTerminals) node among a fired event's subscribers is not
/// itself a root (it computes nothing); instead it promotes the run to include *every* terminal
/// node — the "when this event fires, re-run the whole graph" trigger.
fn collect_roots(
    program: &ExecutionProgram,
    flatten: &FlattenMap,
    seeds: &RunSeeds,
    plan: &mut ExecutionPlan,
) {
    // `plan.reset` already cleared `roots`/`pinned`; this only pushes into them.

    // Node seeds (on-demand preview): roots like any other, plus pinned so their
    // outputs are computed and retained (see `ExecutionPlan::pinned`).
    plan.roots.extend_from_slice(node_roots);
    plan.pinned.extend_from_slice(node_roots);

    // Event subscribers. A `RunTerminals` sink among them fires no cone of its own — it
    // promotes this run to run all terminals (below), so it's skipped as a root here.
    let mut run_terminals = seeds.terminals;
    for event in &seeds.events {
        let e_node = program.e_nodes.by_key(&event.node_id).unwrap();
        let subs = &program.events[e_node.events.range()][event.event_idx].subscribers;
        for &sub in subs {
            if program.e_nodes[sub].special == Some(SpecialNode::RunTerminals) {
                run_terminals = true;
            } else {
                plan.roots.push(sub);
            }
        }
    }

    if !run_terminals && !seeds.event_triggers {
        return;
    }
    // One sweep for both whole-graph seed kinds: terminal nodes (requested directly, or
    // promoted by a fired event reaching a `RunTerminals` sink) and — for the event
    // loop — nodes owning a subscribed event.
    for (idx, e_node) in program.e_nodes.iter().enumerate() {
        if (run_terminals && e_node.terminal)
            || (seeds.event_triggers
                && program.events[e_node.events.range()]
                    .iter()
                    .any(|ev| !ev.subscribers.is_empty()))
        {
            plan.roots.push(idx.into());
        }
    }
}

#[cfg(test)]
mod tests;
