//! Cache-aware refinement of the structural schedule — the "up-to-date check" between
//! [`plan`](crate::execution::plan) and [`execute`](crate::execution::executor). The plan is
//! the static dependency DAG (*what could run*); the [`Resolver`] folds in the current cache
//! state to produce one exact [`ResolvedRun`]: which nodes run or reuse, which outputs they
//! demand, and how many live consumers read each output.
//!
//! This is the split a build system draws between its dependency graph and its dirty /
//! up-to-date analysis (Ninja/Bazel), or a compiler between the CFG and a liveness / dead-code
//! pass: the schedule is structural and reusable across runs, while liveness is per-run and
//! cache-dependent. The reverse sweep visits consumers before producers, probes each needed
//! node against the demand accumulated from running consumers, and stops at cache hits and
//! missing-input nodes.
//!
//! Every node's digest is structural (a fold of its inputs), so the sweep stamps the
//! *whole* graph ahead of the run. The one stamp it can leave imprecise is a digest folding a Bind-delivered
//! *resource* value it can't read yet (`hash_bound_resource`, `digest.rs`): that folds to
//! `None` here — "uncacheable, must run", which keeps the node's cone alive — and the run
//! loop re-stamps it at reach time, once its producers have settled, possibly improving
//! `Run` to a reuse.

use crate::execution::cache::RuntimeCache;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram, OutputIdx};
use crate::execution::{NodeMap, OutputColumn, reset_node_map};
use crate::graph::NodeId;
use crate::node::lambda::OutputDemand;

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

#[derive(Debug, Default)]
pub(crate) struct ResolvedOutputs {
    /// Whether each output must be produced for a live reader or a host pin.
    pub(crate) demand: OutputColumn<OutputDemand>,
    /// Consumers which will actually run and read each output. Pins do not create readers.
    pub(crate) readers: OutputColumn<u32>,
}

impl ResolvedOutputs {
    fn reset(&mut self, output_count: usize) {
        self.demand.reset(output_count, OutputDemand::Skip);
        self.readers.reset(output_count, 0);
    }

    fn seed_external_demand(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        node_id: NodeId,
    ) {
        let outputs = program.e_nodes[&node_id].outputs;
        if plan.pinned.contains(&node_id) {
            self.demand.slice_mut(outputs).fill(OutputDemand::Produce);
            return;
        }
        for port_idx in 0..outputs.len as usize {
            let output_idx = program.output_idx(node_id, port_idx);
            if program.is_output_pinned(output_idx) {
                self.demand[output_idx] = OutputDemand::Produce;
            }
        }
    }

    fn add_reader(&mut self, output_idx: OutputIdx) {
        self.readers[output_idx] = self.readers[output_idx]
            .checked_add(1)
            .expect("output reader count overflowed u32");
        self.demand[output_idx] = OutputDemand::Produce;
    }
}

/// The cache-aware, authoritative shape of one run. All three columns are produced by
/// the same reverse sweep, so a cut/reused/blocked consumer contributes neither demand
/// nor a reader to its producers.
#[derive(Debug, Default)]
pub(crate) struct ResolvedRun {
    pub(crate) disposition: NodeMap<Disposition>,
    pub(crate) outputs: ResolvedOutputs,
}

impl ResolvedRun {
    fn reset(&mut self, program: &ExecutionProgram) {
        reset_node_map(&mut self.disposition, program.node_ids(), Disposition::Cut);
        self.outputs.reset(program.n_outputs());
    }
}

/// Reusable scratch owning the resolved run, so a repeated resolve on an unchanged run
/// allocates nothing — mirroring [`Planner`](crate::execution::plan::Planner). The engine
/// holds one and calls [`resolve`](Self::resolve) each run, between plan and execute.
#[derive(Default, Debug)]
pub(crate) struct Resolver {
    pub(crate) run: ResolvedRun,
}

impl Resolver {
    /// Stamp the structural schedule, then resolve exact liveness and cache reuse.
    /// **Mutates `cache`**: stamps each runnable node's `current_digest` and may flag a
    /// live reusable slot `OnDisk`.
    pub(crate) fn resolve(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut RuntimeCache,
    ) {
        stamp_digests(program, cache, plan);
        resolve_run(program, plan, cache, &mut self.run);
    }
}

/// Producer-first digest pass, so a consumer folds an already-stamped producer digest.
/// Reuse is deliberately not probed here because exact demand exists only in the reverse
/// sweep. A Bind-delivered resource value that is not resident yet stamps `None`; the run
/// loop can improve that node to reuse after its producers settle.
fn stamp_digests(program: &ExecutionProgram, cache: &mut RuntimeCache, plan: &ExecutionPlan) {
    for &node_id in &plan.process_order {
        if !plan.verdicts[&node_id].wants_execute() {
            continue;
        }
        cache.stamp_digest(program, node_id);
    }
}

/// Reverse cache-aware sweep. A running consumer marks exactly the producer ports it reads;
/// reuse and missing-input nodes stop the walk. Producer classification happens only after
/// every downstream consumer has contributed, so cache coverage is checked against exact
/// demand rather than the planner's former structural over-approximation.
fn resolve_run(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    cache: &mut RuntimeCache,
    run: &mut ResolvedRun,
) {
    run.reset(program);
    for node_id in &plan.roots {
        *run.disposition.get_mut(node_id).unwrap() = Disposition::Run;
    }

    for &node_id in plan.process_order.iter().rev() {
        if run.disposition[&node_id] != Disposition::Run {
            continue;
        }
        if !plan.verdicts[&node_id].wants_execute() {
            *run.disposition.get_mut(&node_id).unwrap() = Disposition::Cut;
            continue;
        }
        run.outputs.seed_external_demand(program, plan, node_id);
        let outputs = program.e_nodes[&node_id].outputs;
        let demand = run.outputs.demand.slice(outputs);
        if cache.check_reuse(program, node_id, demand) {
            *run.disposition.get_mut(&node_id).unwrap() = Disposition::Reuse;
            continue;
        }
        *run.disposition.get_mut(&node_id).unwrap() = Disposition::Run;
        for input in &program.inputs[program.e_nodes[&node_id].inputs.range()] {
            if let ExecutionBinding::Bind(addr) = &input.binding {
                *run.disposition.get_mut(&addr.target).unwrap() = Disposition::Run;
                run.outputs
                    .add_reader(program.output_idx(addr.target, addr.port_idx));
            }
        }
    }
}

#[cfg(test)]
mod tests;
