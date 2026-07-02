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
//! The cut is deliberately **structural-only**: a node with a pre-check — or a pre-check
//! ancestor — is [`Resolved::Deferred`], its digest needing a run-time value, so the executor
//! resolves it inline and its cone is never cut. Since every consumer of a `Deferred` node is
//! itself `Deferred`, deferred nodes are never pruned, so the cut only ever touches the
//! pure-structural region where reuse is decidable ahead of the run.

use crate::execution::NodeColumn;
use crate::execution::cache::Cache;
use crate::execution::digest::node_digest;
use crate::execution::output_cache::OutputCache;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::func_lambda::PreCheckDigest;

/// The pre-run structural resolution of one node. It classifies only the *pure-structural*
/// region (nodes whose whole upstream cone is pre-check-free), which is the region where reuse
/// can be decided — and a cone cut is safe — *before* running anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// Not resolved here: the node has a pre-check, or a pre-check ancestor, so its digest
    /// needs a run-time value. The executor's run loop computes it inline (`prepare_node`, the
    /// pre-refactor behavior). Never a reuse hit — and since every consumer of a deferred node
    /// is itself deferred, a deferred node is never cut (its cone always runs).
    #[default]
    Deferred,
    /// Resolved to a cache hit (resident, or a disk blob flagged this run) — its consumers can
    /// reuse it without reading its producers, so a cone feeding *only* reuse hits is pruned.
    Reuse,
    /// Resolved with a stamped digest but no cache hit — the node must run.
    Run,
}

impl Resolved {
    /// A reuse hit doesn't read its producers, so the backward cut stops descending through
    /// it; a `Run`/`Deferred`/missing node does read them, keeping its cone alive.
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

/// Producer-first pass classifying every *pure-structural* node (no pre-check, and no
/// pre-check ancestor) as [`Resolved::Reuse`] or [`Resolved::Run`], stamping its content
/// digest as it goes so a consumer folds an already-stamped producer digest — the
/// producer-first invariant the run loop also relies on. A node carrying a pre-check, or fed
/// by any `Deferred` producer (whose digest isn't settled until it runs), stays
/// [`Resolved::Deferred`]: the run loop resolves it inline and its cone is never cut.
fn resolve_structural(
    program: &ExecutionProgram,
    output_cache: &OutputCache,
    cache: &mut Cache,
    plan: &ExecutionPlan,
    resolved: &mut NodeColumn<Resolved>,
) {
    resolved.reset(program.e_nodes.len(), Resolved::Deferred);
    for &idx in &plan.process_order {
        // A node blocked on a missing required input can't run and isn't a reuse hit; leave it
        // Deferred so the backward cut keeps its cone alive (matches the prior behavior of
        // running everything reachable).
        if !plan.verdicts[idx].wants_execute() {
            continue;
        }
        let e_node = &program.e_nodes[idx];
        // A pre-check keys the node on a run-time value, so its digest can't be resolved here
        // — defer it (this is the structural-only bound on the cut).
        if !e_node.pre_check.is_none() {
            continue;
        }
        // A digest folding a Deferred producer can't be computed structurally (that producer's
        // digest isn't stamped until it runs), so this node is Deferred too.
        let deferred_producer = program.inputs[e_node.inputs.range()].iter().any(|input| {
            matches!(&input.binding, ExecutionBinding::Bind(addr) if resolved[addr.target_idx] == Resolved::Deferred)
        });
        if deferred_producer {
            continue;
        }
        // Pure-structural: fold the digest (reading producers' just-stamped digests) and decide
        // reuse exactly as the run loop's `prepare_node` will.
        let digest = node_digest(program, idx, cache, PreCheckDigest::None);
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
mod tests {
    use super::*;
    use crate::execution::plan::NodeVerdict;
    use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress, NodeIdx};
    use crate::graph::NodeId;
    use crate::prelude::FuncId;
    use common::Span;

    /// Hand-built program for cut tests: each node is `(terminal, &[producer_idx])` — an edge
    /// per listed producer, on its own input port. `compute_needed` reads only the bindings and
    /// the roots, so no digests/lambdas are needed. Node `idx` gets id `from_u128(idx + 1)`.
    #[derive(Default)]
    struct Fix {
        program: ExecutionProgram,
    }

    impl Fix {
        fn node(&mut self, producers: &[NodeIdx]) -> NodeIdx {
            let inputs_start = self.program.inputs.len() as u32;
            for &p in producers {
                self.program.inputs.push(ExecutionInput {
                    required: false,
                    binding: ExecutionBinding::Bind(ExecutionPortAddress {
                        target_idx: p,
                        port_idx: 0,
                    }),
                });
            }
            let idx = self.program.e_nodes.len();
            let outputs_start = self.program.n_outputs as u32;
            self.program.n_outputs += 1;
            self.program.e_nodes.add(ExecutionNode {
                id: NodeId::from_u128(idx as u128 + 1),
                inited: true,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs: Span::new(inputs_start, producers.len() as u32),
                outputs: Span::new(outputs_start, 1),
                ..Default::default()
            });
            idx.into()
        }
    }

    /// Run the backward cut over `fix` with the given roots and per-node resolution.
    fn needed_of(fix: &Fix, roots: &[NodeIdx], resolved: &[Resolved]) -> Vec<bool> {
        let plan = ExecutionPlan {
            // Producer-first order for these fixtures is just index order (a producer is always
            // added before its consumer), matching the planner's post-order.
            process_order: (0..fix.program.e_nodes.len()).map(NodeIdx::from).collect(),
            verdicts: vec![NodeVerdict::Execute; fix.program.e_nodes.len()].into(),
            output_usage: vec![0; fix.program.n_outputs],
            roots: roots.to_vec(),
        };
        let resolved: NodeColumn<Resolved> = resolved.to_vec().into();
        let mut needed = NodeColumn::default();
        compute_needed(&fix.program, &plan, &resolved, &mut needed);
        (0..fix.program.e_nodes.len())
            .map(|i| needed[NodeIdx::from(i)])
            .collect()
    }

    #[test]
    fn reuse_hit_prunes_its_whole_upstream_cone() {
        // src → mid → sink(root). sink runs, mid is a reuse hit ⇒ mid doesn't read src, so
        // both mid's producer (src) is cut while mid itself stays needed (sink reads it).
        let mut f = Fix::default();
        let src = f.node(&[]);
        let mid = f.node(&[src]);
        let sink = f.node(&[mid]);

        let needed = needed_of(
            &f,
            &[sink],
            &[Resolved::Run, Resolved::Reuse, Resolved::Run],
        );
        assert_eq!(
            needed,
            vec![false, true, true],
            "src is cut (its only consumer reused); mid+sink needed"
        );
    }

    #[test]
    fn shared_producer_survives_when_one_consumer_runs() {
        // src feeds a reuse-hit `cached` AND a running `live`; both under the root `sink`.
        // The union must keep src alive for `live` even though `cached` won't read it.
        let mut f = Fix::default();
        let src = f.node(&[]);
        let cached = f.node(&[src]);
        let live = f.node(&[src]);
        let sink = f.node(&[cached, live]);

        let needed = needed_of(
            &f,
            &[sink],
            &[
                Resolved::Run,
                Resolved::Reuse, // cached: won't read src
                Resolved::Run,   // live: reads src
                Resolved::Run,
            ],
        );
        assert_eq!(
            needed,
            vec![true, true, true, true],
            "src survives: the running consumer `live` still reads it"
        );
    }

    #[test]
    fn deferred_node_keeps_its_cone_even_below_a_reuse_hit() {
        // src → deferred(pre-check) → sink(root). A Deferred node reads its producers, so src
        // is never cut — the structural-only bound, even though a hit could sit above.
        let mut f = Fix::default();
        let src = f.node(&[]);
        let deferred = f.node(&[src]);
        let sink = f.node(&[deferred]);

        let needed = needed_of(
            &f,
            &[sink],
            &[Resolved::Run, Resolved::Deferred, Resolved::Run],
        );
        assert_eq!(
            needed,
            vec![true, true, true],
            "a Deferred node reads its producers, so its cone is kept"
        );
    }

    #[test]
    fn cone_reachable_only_through_a_reuse_hit_is_fully_pruned() {
        // deep → src → cached(hit) → sink(root). The whole chain above `cached` is cut.
        let mut f = Fix::default();
        let deep = f.node(&[]);
        let src = f.node(&[deep]);
        let cached = f.node(&[src]);
        let sink = f.node(&[cached]);

        let needed = needed_of(
            &f,
            &[sink],
            &[Resolved::Run, Resolved::Run, Resolved::Reuse, Resolved::Run],
        );
        assert_eq!(
            needed,
            vec![false, false, true, true],
            "both deep and src are pruned behind the reuse hit"
        );
    }
}
