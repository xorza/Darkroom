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
//! the run loop. [`compute_disposition`] is literally a mark-sweep from the run's roots that
//! stops descending at a cache hit.
//!
//! Every node's digest is structural (a fold of its inputs), so the sweep stamps the
//! *whole* graph ahead of the run. The one stamp it can leave imprecise is a digest folding a Bind-delivered
//! *resource* value it can't read yet (`hash_bound_resource`, `digest.rs`): that folds to
//! `None` here — "uncacheable, must run", which keeps the node's cone alive — and the run
//! loop re-stamps it at reach time, once its producers have settled, possibly improving
//! `Run` to a reuse.

use crate::execution::cache::RuntimeCache;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::execution::{NodeMap, NodeSet, reset_node_map};

/// What the run loop does with one node — the resolver's single exposed column, merging the
/// reuse verdict with the backward cut so the three states are mutually exclusive by
/// construction (a pruned reuse hit can't also read as `Reuse`). Authoritative for the whole
/// run: a `Reuse` is never re-derived by the executor, since a node digest folds live
/// external state (`FsPath` len/mtime, resource stamps) and a second derivation mid-run
/// could flip a verdict after the cut already pruned its producers. The safe direction is
/// allowed once: the run loop re-stamps a `Run` node whose digest folded to `None` here (a
/// Bind-delivered resource value not yet readable) — its cone was kept alive, so improving
/// it to a reuse at reach time contradicts nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Pruned by the cut: no running node reads this node this run. The default — a node
    /// the sweep never promotes stays cut.
    #[default]
    Cut,
    /// An unchanged output is cached (resident in RAM, or a blob on disk) — serve it
    /// without running the lambda.
    Reuse,
    /// The node must run.
    Run,
}

impl Disposition {
    /// Whether some running node will read this node — the active frontier; [`Cut`
    /// ](Disposition::Cut) is the only state off it.
    pub(crate) fn needed(self) -> bool {
        self != Disposition::Cut
    }
}

/// Reusable scratch owning the resolve columns, so a repeated resolve on an unchanged run
/// allocates nothing — mirroring [`Planner`](crate::execution::plan::Planner). The engine
/// holds one and calls [`resolve`](Self::resolve) each run, between plan and execute.
#[derive(Default, Debug)]
pub(crate) struct Resolver {
    /// Pass-1 cache-hit scratch, read when the sweep promotes a node onto the frontier.
    /// Internal — everything downstream reads `disposition`.
    reused: NodeSet,
    /// The run's authoritative per-node verdicts (see [`Disposition`]), read by the
    /// executor's run loop and the end-of-run eviction.
    pub(crate) disposition: NodeMap<Disposition>,
}

impl Resolver {
    /// Resolve every scheduled node's reuse verdict and sweep the backward cut, merging
    /// the two into [`disposition`](Self::disposition). **Mutates `cache`**: stamps each
    /// pure-structural node's `current_digest` and may flag a slot `OnDisk`.
    pub(crate) fn resolve(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut RuntimeCache,
    ) {
        resolve_structural(program, cache, plan, &mut self.reused);
        compute_disposition(program, plan, &self.reused, &mut self.disposition);
    }
}

/// Producer-first pass recording cache hits while stamping each node's content digest, so a
/// consumer folds an already-stamped producer digest — the producer-first invariant the run
/// loop also relies on. Nearly every digest is a structural fold and resolves here; one folding a
/// Bind-delivered resource value it can't read yet stamps `None` — `Run`, cone kept alive —
/// and the run loop re-stamps it at reach time (see the module doc).
fn resolve_structural(
    program: &ExecutionProgram,
    cache: &mut RuntimeCache,
    plan: &ExecutionPlan,
    reused: &mut NodeSet,
) {
    reused.clear();
    for &node_id in &plan.process_order {
        // A node blocked on a missing required input can't run and isn't a reuse hit; leave it
        // `Run` so the backward cut keeps its cone alive.
        if !plan.verdicts[&node_id].wants_execute() {
            continue;
        }
        // Fold the digest (reading producers' just-stamped digests) and decide reuse — the
        // one verdict for the run; the run loop reads the merged `disposition` rather than
        // re-deriving.
        let demand = plan.outputs.demand.slice(program.e_nodes[&node_id].outputs);
        if cache.stamp_and_check_reuse(program, node_id, demand) {
            reused.insert(node_id);
        }
    }
}

/// Backward cut fused with the verdict merge: every node starts [`Disposition::Cut`], the
/// walk roots are promoted to their cache-hit/run verdict, then `process_order` is swept in
/// reverse (consumers before producers, since it's producer-first) promoting the producers
/// of every *running* node. A reused node reads none of its producers (it
/// serves a cache), so it promotes nothing — a cone feeding only reuse hits stays `Cut`. A
/// producer read by *any* running consumer is promoted (this union is why the cut must be a
/// separate backward pass, not a forward filter over the resolution).
fn compute_disposition(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    reused: &NodeSet,
    disposition: &mut NodeMap<Disposition>,
) {
    fn promote(reused: bool) -> Disposition {
        if reused {
            Disposition::Reuse
        } else {
            Disposition::Run
        }
    }
    reset_node_map(disposition, program.node_ids(), Disposition::Cut);
    for &root in &plan.roots {
        *disposition.get_mut(&root).unwrap() = promote(reused.contains(&root));
    }
    for &node_id in plan.process_order.iter().rev() {
        // Only a running node reads its producers: `Reuse` serves a cache and `Cut` is
        // never read, so neither passes need upstream.
        if disposition[&node_id] != Disposition::Run {
            continue;
        }
        for input in &program.inputs[program.e_nodes[&node_id].inputs.range()] {
            if let ExecutionBinding::Bind(addr) = &input.binding {
                *disposition.get_mut(&addr.target).unwrap() =
                    promote(reused.contains(&addr.target));
            }
        }
    }
}

#[cfg(test)]
mod tests;
