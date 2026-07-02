//! Cache-aware refinement of the structural schedule — the "up-to-date check" between
//! [`plan`](crate::execution::plan) and [`execute`](crate::execution::executor). The plan is
//! the static dependency DAG (*what could run*); the [`Resolver`] folds in the current cache
//! state to decide *what still needs to*: which pure-structural nodes reuse a cached output,
//! and — pruning every cone that feeds only reuse hits — which nodes a running node will
//! actually read.
//!
//! This is the split a build system draws between its dependency graph and its dirty /
//! up-to-date analysis (Ninja/Bazel), or a compiler between the CFG and a liveness / dead-code
//! pass: the schedule is structural and reusable across runs, the cut is per-run and
//! cache-dependent, so it lives here rather than muddying the (purely structural) planner or
//! the run loop. [`compute_needed`] is literally a mark-sweep from the run's roots that stops
//! descending at a cache hit.
//!
//! Every node's digest is either structural (a fold of its inputs) or a contained special
//! case (the file-cache node keys on its path), so the sweep can resolve the *whole* graph
//! ahead of the run — there is nothing it must defer.

use crate::execution::NodeColumn;
use crate::execution::cache::Cache;
use crate::execution::digest::node_digest;
use crate::execution::output_cache::OutputCache;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};

/// The pre-run resolution of one node: does its cached output reuse this run, or must it run?
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// Resolved to a cache hit (resident, or a disk blob flagged this run) — its consumers can
    /// reuse it without reading its producers, so a cone feeding *only* reuse hits is pruned.
    Reuse,
    /// A stamped digest but no cache hit — the node must run. The default: a node outside
    /// `process_order` (never resolved) reads as `Run`, the conservative "keep its cone alive".
    #[default]
    Run,
}

impl Resolved {
    /// A reuse hit doesn't read its producers, so the backward cut stops descending through
    /// it; a `Run` (or missing) node does read them, keeping its cone alive.
    fn reads_producers(self) -> bool {
        self != Resolved::Reuse
    }
}

/// Reusable scratch owning the cut columns, so a repeated resolve on an unchanged run
/// allocates nothing — mirroring [`Planner`](crate::execution::plan::Planner). The engine
/// holds one and calls [`resolve`](Self::resolve) each run, between plan and execute.
#[derive(Default, Debug)]
pub(crate) struct Resolver {
    /// Per-node structural resolution (see [`Resolved`]); internal to the cut.
    resolved: NodeColumn<Resolved>,
    /// The cut mask handed to the executor: `true` for a node some running node will read.
    needed: NodeColumn<bool>,
}

impl Resolver {
    /// Resolve reuse and compute the cut mask, returning `needed` (`true` = some running node
    /// will read this node; `false` = pruned, because its consumers all reused). **Mutates
    /// `cache`**: stamps each pure-structural node's `current_digest` and may flag a slot
    /// `OnDisk`; the executor re-derives each surviving node's digest idempotently in its run
    /// loop, so the reuse decision can't drift between the two.
    pub(crate) fn resolve(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut Cache,
        output_cache: &OutputCache,
    ) -> &NodeColumn<bool> {
        resolve_structural(program, output_cache, cache, plan, &mut self.resolved);
        compute_needed(program, plan, &self.resolved, &mut self.needed);
        &self.needed
    }
}

/// Producer-first pass classifying every node as [`Resolved::Reuse`] or [`Resolved::Run`],
/// stamping its content digest as it goes so a consumer folds an already-stamped producer
/// digest — the producer-first invariant the run loop also relies on. Every digest is a
/// structural fold (or the file-cache node's path key), so the whole graph resolves here.
fn resolve_structural(
    program: &ExecutionProgram,
    output_cache: &OutputCache,
    cache: &mut Cache,
    plan: &ExecutionPlan,
    resolved: &mut NodeColumn<Resolved>,
) {
    resolved.reset(program.e_nodes.len(), Resolved::Run);
    for &idx in &plan.process_order {
        // A node blocked on a missing required input can't run and isn't a reuse hit; leave it
        // `Run` so the backward cut keeps its cone alive.
        if !plan.verdicts[idx].wants_execute() {
            continue;
        }
        // Fold the digest (reading producers' just-stamped digests) and decide reuse exactly
        // as the run loop's `prepare_node` will.
        let digest = node_digest(program, idx, cache);
        cache.slots[idx].current_digest = digest;
        let hit = digest.is_some()
            && (cache.is_resident_hit(idx)
                || output_cache.mark_on_disk_if_present(program, idx, cache));
        resolved[idx] = if hit { Resolved::Reuse } else { Resolved::Run };
    }
}

/// Backward cut: seed `needed` with the walk roots, then sweep `process_order` in reverse
/// (consumers before producers, since it's producer-first) propagating need through every node
/// that will *read* its producers. A [`Resolved::Reuse`] node reads none (it serves a cache),
/// so it passes no need to its producers — a cone feeding only reuse hits stays `false` and the
/// run loop prunes it. A producer needed by *any* reading consumer stays alive (this union is
/// why the cut must be a separate backward pass, not a forward filter over the resolution).
fn compute_needed(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    resolved: &NodeColumn<Resolved>,
    needed: &mut NodeColumn<bool>,
) {
    needed.reset(program.e_nodes.len(), false);
    for &root in &plan.roots {
        needed[root] = true;
    }
    for &idx in plan.process_order.iter().rev() {
        if !needed[idx] || !resolved[idx].reads_producers() {
            continue;
        }
        for input in &program.inputs[program.e_nodes[idx].inputs.range()] {
            if let ExecutionBinding::Bind(addr) = &input.binding {
                needed[addr.target_idx] = true;
            }
        }
    }
}

#[cfg(test)]
mod tests;
