//! The structural pass: one backward post-order DFS from the run's roots that
//! fills a [`RunSchedule`]'s order and per-node verdicts. The scratch it walks
//! with — the entered marks and the work stack — lives on the [`Planner`] and
//! is kept across runs, so a repeated plan on an unchanged graph allocates
//! nothing.

use crate::containers::set::IdxSet;
use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionBinding};
use crate::execution::error::{Error, Result};
use crate::execution::index::NodeIdx;
use crate::execution::schedule::{NodeState, RunSchedule};
use crate::execution::seeds::RunSeeds;

#[derive(Debug)]
enum Visit {
    Discover(NodeIdx),
    Done(NodeIdx),
}

/// Reusable per-run scheduling scratch, kept across runs so a repeated plan on
/// an unchanged graph does no scheduling allocations.
///
/// The walk's three colors are read off the schedule: a node is done (black)
/// once its state left `Unvisited`, on the stack (gray) when entered but not
/// done, and unvisited (white) otherwise. Only "entered" is the walk's own.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    /// The nodes the walk has entered, whose `Done` visit is pushed.
    entered: IdxSet<NodeIdx>,
    /// DFS work stack.
    stack: Vec<Visit>,
}

impl Planner {
    fn reset_for_program(&mut self, program: &CompiledGraph) {
        self.stack.clear();
        self.entered.reset(program.e_nodes.len());
    }

    /// Build the per-run schedule into `schedule` from the installed program and the run's
    /// `seeds` (the roots to walk back from). Exact execution-node seeds are roots
    /// directly. Errors on a dependency cycle or a node/event seed absent from the program.
    ///
    /// Leaves `schedule` structurally complete and cache-blind:
    /// [`RunSchedule::resolve`] is the pass that refines it, and running the two out
    /// of order is what [`RunSchedule::validate`] catches.
    pub(crate) fn plan(
        &mut self,
        program: &CompiledGraph,
        seeds: &RunSeeds,
        schedule: &mut RunSchedule,
    ) -> Result<()> {
        schedule.reset_for_program(program);
        self.reset_for_program(program);

        // Collect the walk roots straight into `schedule.roots` — they seed the
        // backward walk below and the cache-aware reverse sweep.
        schedule.collect_roots(program, seeds)?;

        self.walk_backward_collect_order(program, schedule)?;
        schedule.validate_debug(program);
        Ok(())
    }

    /// Backward post-order DFS from the roots: builds `process_order` (deps before
    /// consumers), detects cycles, and resolves each node's structural [`NodeState`].
    /// The state is set in the `Done` arm, i.e. in post-order, so every `Bind`
    /// producer is done, with its own state set, when a consumer reads it.
    fn walk_backward_collect_order(
        &mut self,
        program: &CompiledGraph,
        schedule: &mut RunSchedule,
    ) -> Result<()> {
        for &node_idx in schedule.roots() {
            self.stack.push(Visit::Discover(node_idx));
        }

        while let Some(visit) = self.stack.pop() {
            let node_idx = match visit {
                Visit::Discover(node_idx) => node_idx,
                Visit::Done(node_idx) => {
                    debug_assert!(self.entered.contains(node_idx));
                    debug_assert_eq!(schedule.states[node_idx], NodeState::Unvisited);
                    schedule.process_order.push(node_idx);
                    // Runnable unless a required input is unbound or fed by a
                    // non-runnable producer. Post-order ⇒ deps already verdicted, so
                    // `input_missing` reads settled values. Whether the node's output is
                    // reused from cache is decided at execution, not here.
                    let missing = program.inputs[program[node_idx].inputs]
                        .iter()
                        .any(|e_input| schedule.input_missing(program, e_input));
                    // `Cut` is the planner's *positive* verdict — runnable, and
                    // nothing has claimed it yet. The cache-aware sweep promotes
                    // the ones a running consumer reads and leaves the rest here.
                    schedule.states[node_idx] = if missing {
                        NodeState::MissingInputs
                    } else {
                        NodeState::Cut
                    };
                    continue;
                }
            };

            if schedule.states[node_idx] != NodeState::Unvisited {
                continue;
            }
            if self.entered.contains(node_idx) {
                return Err(Error::CycleDetected {
                    node_id: program.node_ids[node_idx],
                });
            }

            let e_node = &program[node_idx];
            // Disabled nodes block dependency traversal, but an explicit node
            // seed is recorded before this walk and overrides disable for this run.
            if e_node.disabled && !schedule.root_flags(node_idx).is_seeded() {
                schedule.states[node_idx] = NodeState::Disabled;
                continue;
            }

            self.entered.insert(node_idx);
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
