//! Cache-aware refinement of the structural schedule — the "up-to-date check" between
//! [`plan`](crate::execution::plan) and [`execute`](crate::execution::executor). The plan is
//! the static dependency DAG (*what could run*); the [`Resolver`] folds in the current cache
//! state to produce one exact resolved run: which nodes run or reuse, which outputs they
//! demand, and how many live consumers read each output.
//!
//! This is the split a build system draws between its dependency graph and its dirty /
//! up-to-date analysis (Ninja/Bazel), or a compiler between the CFG and a liveness / dead-code
//! pass: the schedule is structural and reusable across runs, while liveness is per-run and
//! cache-dependent. The reverse sweep visits consumers before producers, probes each needed
//! node against the demand accumulated from running consumers, and stops at cache hits,
//! missing-input nodes, and funcs without an implementation.
//!
//! Every node's digest is structural (a fold of its inputs and the run's prepared filesystem
//! snapshot), so the sweep stamps the *whole* graph ahead of the run. The one stamp it can
//! leave imprecise is a digest folding a Bind-delivered path value it can't read yet:
//! that folds to `None` here — "uncacheable, must run", which keeps the node's cone alive —
//! and the run loop prepares the identity and re-stamps at reach time once its producers
//! have settled, possibly improving `Run` to a reuse.

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::index::{NodeColumn, OutputColumn, OutputIdx};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::node::lambda::OutputDemand;

/// What the run loop does with one node — the resolver's single exposed column, merging the
/// reuse verdict with the backward cut so the states are mutually exclusive by
/// construction (a pruned reuse hit can't also read as `Reuse`). Authoritative for the whole
/// run: a `Reuse` is never re-derived after the cut may have pruned its producers. The safe
/// direction is allowed once: the run loop prepares and re-stamps a `Run` node whose bound
/// path value was not yet readable — its cone was kept alive, so improving it to reuse
/// at reach time contradicts nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Pruned by the cut: no running node reads this node this run. The default — a node
    /// the sweep never promotes stays cut.
    #[default]
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

#[derive(Debug, Default)]
pub(crate) struct ResolvedOutputs {
    /// Whether each output must be produced for a live reader or a host pin.
    pub(crate) demand: OutputColumn<OutputDemand>,
    /// Consumers which will actually run and read each output.
    pub(crate) readers: OutputColumn<u32>,
}

impl ResolvedOutputs {
    fn reset(&mut self, output_count: usize) {
        self.demand.reset(output_count, OutputDemand::Skip);
        self.readers.reset(output_count, 0);
    }

    fn add_reader(&mut self, output_idx: OutputIdx) {
        let readers = &mut self.readers[output_idx];
        debug_assert_ne!(*readers, u32::MAX, "output reader count overflowed u32");
        *readers = readers.wrapping_add(1);
        self.demand[output_idx] = OutputDemand::Produce;
    }
}

/// The cache-aware, authoritative shape of one run. All three columns are produced by
/// the same reverse sweep, so a cut/reused/blocked consumer contributes neither demand
/// nor a reader to its producers.
#[derive(Debug, Default)]
pub(crate) struct Resolver {
    pub(crate) disposition: NodeColumn<Disposition>,
    pub(crate) outputs: ResolvedOutputs,
}

impl Resolver {
    /// Stamp the structural schedule, then sweep it in reverse for exact liveness and
    /// cache reuse.
    ///
    /// **Mutates `cache`** only to stamp each runnable node's `current_digest`: a live
    /// disk-cache frontier is *probed* from its blob header here and decoded later, by the
    /// run loop, when it reaches the node.
    ///
    /// In the sweep, a running consumer marks exactly the producer ports it reads; reuse,
    /// missing-input nodes, and missing lambdas stop the walk. Producer classification
    /// happens only after every downstream consumer has contributed, so cache coverage is
    /// checked against exact demand rather than the planner's structural
    /// over-approximation.
    pub(crate) async fn resolve(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut RuntimeCache,
    ) {
        cache.stamp_digests(program, plan);

        self.disposition
            .reset(program.e_nodes.len(), Disposition::Cut);
        self.outputs.reset(program.outputs.len());

        for node_idx in plan.roots.iter() {
            self.disposition[node_idx] = Disposition::Run;
        }

        for &node_idx in plan.process_order.iter().rev() {
            if self.disposition[node_idx] != Disposition::Run {
                continue;
            }
            if !plan.verdicts[node_idx].wants_execute() {
                self.disposition[node_idx] = Disposition::Cut;
                continue;
            }
            let e_node = &program[node_idx];
            if e_node.lambda.is_none() {
                self.disposition[node_idx] = Disposition::MissingLambda;
                continue;
            }
            let outputs = e_node.outputs;
            // A node seed ("run to this node") demands every output the node has:
            // the host asked for the node itself, not for what a consumer reads.
            if plan.seeded.contains(node_idx) {
                self.outputs
                    .demand
                    .slice_mut(outputs)
                    .fill(OutputDemand::Produce);
            }
            let demand = self.outputs.demand.slice(outputs);
            if !plan.event_sources.contains(node_idx)
                && cache.probe_reuse(program, node_idx, demand).await
            {
                self.disposition[node_idx] = Disposition::Reuse;
                continue;
            }
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    // Only a producer the plan will actually run can deliver
                    // a value. A **disabled** producer feeding an *optional*
                    // input leaves the consumer perfectly schedulable —
                    // `input_missing` treats an optional port fed by a
                    // disabled producer as satisfied — but the producer
                    // itself never enters `process_order`. Marking it live
                    // here put a node the schedule does not contain into the
                    // run, and the consumer's read then demanded an output
                    // nothing would ever produce: a panic on a cold cache,
                    // and on a warm one the value from before it was
                    // disabled, served as if it were this run's.
                    if !plan.verdicts[addr.node_idx].wants_execute() {
                        continue;
                    }
                    self.disposition[addr.node_idx] = Disposition::Run;
                    self.outputs.add_reader(program.output_idx(*addr));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
