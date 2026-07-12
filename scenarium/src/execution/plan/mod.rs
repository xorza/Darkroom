//! Scheduling: the per-run schedule ([`ExecutionPlan`]) and the [`Planner`] that builds
//! it. The planner runs one backward post-order DFS from the run's roots (sinks,
//! event subscribers, event-trigger owners — plus every sink when a fired event
//! reaches a [`RunSinks`](crate::node::special::SpecialNode::RunSinks) sink),
//! producing `process_order` (deps before consumers), per-output usage counts, and each
//! node's [`NodeVerdict`] (runnable vs blocked on inputs) — purely structural, no
//! cache/digest state. The
//! [`Executor`](crate::execution::executor::Executor) consumes the plan; the plan is
//! reused via a buffer on the engine and the `Planner` owns reusable DFS scratch, so a
//! repeated plan on an unchanged graph allocates nothing.

use crate::execution::compile::CompiledGraph;
use crate::execution::program::{ExecutionBinding, ExecutionInput, ExecutionProgram, NodeIdx};
use crate::execution::query::resolve_node_idx;
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
    /// seeded from the sinks — every reachable node, producer-first. The executor
    /// walks this and skips `MissingInputs` nodes (and reuses cached ones) inline.
    pub(crate) process_order: Vec<NodeIdx>,
    /// Per-node verdict (execute / missing-inputs), indexed by node position.
    pub(crate) verdicts: NodeColumn<NodeVerdict>,
    /// Per-output usage, indexed by output-pool index: in-graph consumer counts from
    /// the backward walk, plus one extra unit for a port with a reader outside the
    /// schedule — a port the compiled program flags externally bound (a GUI inspector
    /// reading it live), or one belonging to a pinned root (the preview fetch reads it
    /// after the run) — folded in here (see [`Planner::plan`]) so this is the single,
    /// complete source of truth: the executor maps it straight to
    /// [`OutputUsage`](crate::node::func_lambda::OutputUsage), never touching the
    /// compiled program's or plan's own pools again. Both are `+= 1`, not a floor to
    /// `1`: a port with `n` real consumers must land at `n + 1`, or the executor's
    /// move-on-last-use optimization would take the value out from under the extra
    /// reader on the last *real* read (it only fires when a read leaves the count at
    /// exactly zero). `> 0` ⇒ the output is `Needed` this run; `0` ⇒ `Skip`.
    pub(crate) output_usage: Vec<u32>,
    /// The nodes the backward walk started from — sinks, event subscribers,
    /// event-trigger owners, and node seeds (a node seeding via several categories may
    /// repeat; harmless). The schedule's "must be available" set: the executor's pre-run
    /// cut seeds its `needed` mask from these and prunes any cone reachable only through
    /// cache-hit consumers (see [`Executor`](crate::execution::executor::Executor)).
    pub(crate) roots: Vec<NodeIdx>,
    /// The node-seeded roots (on-demand preview targets), a subset of `roots`. Drives two
    /// things: `plan.output_usage` gets a `+= 1` for each pinned node's outputs (above),
    /// and the executor's per-run retention policy (`Executor::retain`) keeps the node's
    /// outputs resident through every release/eviction site whatever its cache mode.
    /// Retention is all it takes for a repeated run to be a RAM hit: the reuse check
    /// serves any resident digest-valid value.
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
    pub(crate) fn reset(&mut self, n_nodes: usize) {
        self.process_order.clear();
        self.verdicts.reset(n_nodes, NodeVerdict::default());
        self.output_usage.clear();
        self.roots.clear();
        self.pinned.clear();
    }

    /// Seed `output_usage` with every unit of usage that comes from outside the
    /// schedule itself, before [`Planner::plan`]'s backward walk adds each in-graph
    /// consumer's own count on top: one unit for a port the compiled program flags
    /// externally bound (a GUI inspector reading it live), and one for each output of a
    /// pinned root (`self.pinned`, already populated by `collect_roots` by the time
    /// this runs) — its value is read by the preview fetch after the run. Both are
    /// `+= 1`, never a floor: a port with `n` real consumers must land at `n + 1`, or
    /// the executor's move-on-last-use optimization (`collect_inputs`) would take the
    /// value out of its slot on the last *real* read (it only fires when a read leaves
    /// the usage at exactly zero). Folding both in here, once, makes `output_usage` the
    /// single, complete source of truth — the executor maps it straight to
    /// `OutputUsage` and never cross-references the compiled program's pool or
    /// `self.pinned` again for this.
    ///
    /// Called right after `reset` zero-filled `output_usage` (nothing has run the
    /// backward walk yet), so the external-binding half is a plain assignment; pinning
    /// then adds on top, so a port that's both pinned and externally bound correctly
    /// lands at `2`.
    pub(crate) fn seed_extra_usage(&mut self, program: &ExecutionProgram) {
        // `output_usage`'s length comes from the `extend` below, not a separate resize
        // in `reset` — so what actually needs checking isn't "these two already agree"
        // (they can't yet: nothing has sized `output_usage` before this runs), it's that
        // the compiled program's own pool has exactly one entry per pooled output port.
        // `Flattener::build` asserts this for a real compile; `Fix::node` keeps it true
        // for this module's hand-built tests.
        assert_eq!(
            program.output_external_bindings.len(),
            program.n_outputs(),
            "output_external_bindings must have exactly one entry per pooled output port"
        );
        // `+=` below (in the pinned loop) only makes sense against a freshly `reset`
        // (empty) column — a double call would silently double-count rather than fail
        // loudly, so assert the contract this fn and `Planner::plan`'s call site rely on.
        assert!(
            self.output_usage.is_empty(),
            "seed_extra_usage must run on a freshly reset output_usage column"
        );
        self.output_usage
            .extend(program.output_external_bindings.iter().map(|&b| b as u32));

        for &idx in &self.pinned {
            for usage in &mut self.output_usage[program.e_nodes[idx].outputs.range()] {
                *usage += 1;
            }
        }
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
        plan.reset(program.e_nodes.len());

        // Collect the walk roots straight into `plan.roots` — they seed the backward walk
        // below *and* the executor's pre-run cut, so they live on the plan as an output.
        // Must run *before* `seed_extra_usage`: that's what populates `plan.pinned`, which
        // `seed_extra_usage`'s pinned fold reads.
        collect_roots(compiled, seeds, plan)?;

        // Both non-schedule usage sources (external bindings, pinned roots) are folded
        // in together here, before the walk below adds each in-graph consumer's own
        // count on top.
        plan.seed_extra_usage(program);

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
    for &id in &seeds.nodes {
        let idx = resolve_node_idx(compiled, &id).ok_or(Error::NodeSeedNotFound { node_id: id })?;
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
