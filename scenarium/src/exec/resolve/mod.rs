//! The cache-aware sweep between [`plan`](crate::exec::schedule::planner::Planner::plan)
//! and [`execute`](crate::exec::executor): the "up-to-date check" that refines a planned
//! [`RunSchedule`] against the [`RuntimeCache`]. The planner's answer is *what could run*;
//! this one is *what will*.
//!
//! Its own module because it reads both: the schedule stays structural and the cache knows
//! nothing of liveness, so neither has to depend on the other.

use crate::exec::cache::runtime_cache::RuntimeCache;
use crate::exec::compile::compiled_graph::CompiledGraph;
use crate::exec::schedule::{Delivery, NodeState, RunSchedule};
use crate::graph::func::lambda::OutputDemand;

/// Stamp the planned `schedule`, then sweep it in reverse for exact liveness and cache
/// reuse, writing the result into that same buffer: the [`NodeState`] column the planner
/// opened, plus the `outputs` half beside it.
///
/// **Mutates `cache`** only to stamp each runnable node's `cur_digest`: a live
/// disk-cache frontier is *probed* from its blob header here and decoded later, by the
/// run loop, when it reaches the node.
///
/// In the sweep, a running consumer marks exactly the producer ports it reads; reuse,
/// missing-input nodes, and missing lambdas stop the walk. Producer classification
/// happens only after every downstream consumer has contributed, so cache coverage is
/// checked against exact demand rather than the planner's structural
/// over-approximation.
///
/// Every node's digest is structural (a fold of its inputs and the run's prepared
/// filesystem snapshot), so this stamps the *whole* graph ahead of the run. The one
/// stamp it can leave imprecise is a digest folding a Bind-delivered path value it
/// can't read yet: that folds to `None` here — "uncacheable, must run", which keeps
/// the node's cone alive — and the run loop prepares the identity and re-stamps at
/// reach time once its producers have settled, possibly improving `Run` to a reuse.
pub(crate) async fn sweep(
    program: &CompiledGraph,
    schedule: &mut RunSchedule,
    cache: &mut RuntimeCache,
) {
    // The cache holds no program of its own, so every question below names
    // the one `program` this schedule was planned against.
    cache.stamp_digests(program, schedule);
    // The sweep *accumulates* demand and readers, so it starts from zero of
    // its own accord rather than trusting whoever opened the schedule.
    schedule.outputs.reset(program.output_count as usize);
    schedule.promote_roots();

    // Reverse process order, so every consumer of a node has contributed
    // its demand before the node is classified: a producer is promoted and
    // read-counted by each live consumer on the way past, and only then
    // asked whether a cache covers what was actually demanded. That also
    // means a promotion can only overwrite the planner's `Cut` or an
    // earlier consumer's `Run`, never a `Reuse` this sweep settled.
    for &node_idx in schedule.process_order.iter().rev() {
        // Reaching this body means the planner cleared the node and some
        // consumer (or a root) promoted it — `Run` is written no other way.
        if schedule.states[node_idx] != NodeState::Run {
            continue;
        }
        let e_node = &program[node_idx];

        let flags = schedule.root_flags(node_idx);
        let demand = &mut schedule.outputs.demand[e_node.outputs];
        // A node seed ("run to this node") demands every output the node has:
        // the host asked for the node itself, not for what a consumer reads.
        if flags.is_seeded() {
            demand.fill(OutputDemand::Produce);
        }
        if !flags.is_event_source() && cache.probe_reuse(program, node_idx, demand).await {
            schedule.states[node_idx] = NodeState::Reuse;
            continue;
        }

        // A read is planned exactly where the run loop will take one: on a
        // `Wire` delivery, whose producer the plan runs. A **disabled**
        // producer feeding an *optional* input leaves the consumer
        // schedulable, but never enters `process_order`, so it delivers
        // nothing and is not promoted.
        for input in &program.inputs[e_node.inputs] {
            if let Delivery::Wire(addr) = schedule.delivery(input) {
                let promoted = schedule.states[addr.node_idx].promote_to_run();
                debug_assert!(promoted, "a delivering producer is runnable");
                schedule.outputs.add_reader(program.output_idx(addr));
            }
        }
    }

    schedule.validate_debug(program);
}

#[cfg(test)]
mod tests;
