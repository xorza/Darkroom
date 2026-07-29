//! Cache-aware refinement of the structural schedule — the "up-to-date check" between
//! [`plan`](crate::execution::plan) and [`execute`](crate::execution::executor). The plan is
//! the static dependency DAG (*what could run*); the [`Resolver`] folds in the current cache
//! state to produce one exact resolved run: which nodes run or reuse, which outputs they
//! demand, and how many live consumers read each output.
//!
//! This is the split a build system draws between its dependency graph and its dirty /
//! up-to-date analysis (Ninja/Bazel), or a compiler between the CFG and a liveness / dead-code
//! pass: the schedule is structural, the liveness cache-dependent. The two are *passes*, not
//! two pieces of state — every run re-plans, so the refinement happens in the plan's own
//! [`NodeState`] column rather than a second one shadowing it. The reverse sweep visits
//! consumers before producers, probes each needed
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
use crate::execution::plan::{ExecutionPlan, NodeState};
use crate::execution::program::index::{OutputColumn, OutputIdx};
use crate::execution::program::{ExecutionBinding, Program};
use crate::node::lambda::OutputDemand;

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

/// The per-output half of one resolved run — the per-node half is the plan's
/// own [`NodeState`] column, which this sweep refines in place. All three are
/// produced by the same reverse pass, so a cut/reused/blocked consumer
/// contributes neither demand nor a reader to its producers.
#[derive(Debug, Default)]
pub(crate) struct Resolver {
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
        program: &Program,
        plan: &mut ExecutionPlan,
        cache: &mut RuntimeCache,
    ) {
        cache.stamp_digests(program, plan.executing());
        self.outputs.reset(program.outputs.len());

        // Destructured so the sweep can read the schedule and the seed sets
        // while writing the state column — disjoint fields of the one plan.
        let ExecutionPlan {
            process_order,
            states,
            roots,
            seeded,
            event_sources,
        } = plan;

        for node_idx in roots.iter() {
            // Only a root the planner cleared. Promoting a `Disabled` or
            // `MissingInputs` root would overwrite the verdict the run's
            // outcome reports it by — and claim a node the schedule may not
            // even contain.
            if states[node_idx].is_runnable() {
                states[node_idx] = NodeState::Run;
            }
        }

        for &node_idx in process_order.iter().rev() {
            // `Run` is written only through an `is_runnable` gate — here and
            // at the producer promotion below — so reaching this body already
            // means the planner cleared the node. There is no second check.
            if states[node_idx] != NodeState::Run {
                continue;
            }
            let e_node = &program[node_idx];
            if e_node.lambda.is_none() {
                states[node_idx] = NodeState::MissingLambda;
                continue;
            }
            let outputs = e_node.outputs;
            // A node seed ("run to this node") demands every output the node has:
            // the host asked for the node itself, not for what a consumer reads.
            if seeded.contains(node_idx) {
                self.outputs
                    .demand
                    .slice_mut(outputs)
                    .fill(OutputDemand::Produce);
            }
            let demand = self.outputs.demand.slice(outputs);
            if !event_sources.contains(node_idx)
                && cache.probe_reuse(program, node_idx, demand).await
            {
                states[node_idx] = NodeState::Reuse;
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
                    //
                    // Reverse order also means a producer is promoted before
                    // it is classified, so this only ever overwrites the
                    // planner's `Cut` or an earlier consumer's `Run` — never
                    // a `Reuse` this sweep already settled.
                    if !states[addr.node_idx].is_runnable() {
                        continue;
                    }
                    states[addr.node_idx] = NodeState::Run;
                    self.outputs.add_reader(program.output_idx(*addr));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
