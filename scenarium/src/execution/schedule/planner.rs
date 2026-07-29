//! The structural pass: one backward post-order DFS from the run's roots that
//! fills a [`RunSchedule`]'s order and per-node verdicts. The scratch it walks
//! with — the coloring and the work stack — lives on the [`Planner`] and is kept
//! across runs, so a repeated plan on an unchanged graph allocates nothing.

use crate::execution::error::{Error, Result};
use crate::execution::program::index::{NodeColumn, NodeIdx};
use crate::execution::program::{ExecutionBinding, Program};
use crate::execution::schedule::{NodeState, RunSchedule, Scheduled};
use crate::execution::seeds::RunSeeds;

/// DFS coloring for the backward pass. White = unvisited, Gray = on
/// stack (Done pushed, children pending), Black = children done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Color {
    #[default]
    White,
    Gray,
    Black,
}

#[derive(Debug)]
enum Visit {
    Discover(NodeIdx),
    Done(NodeIdx),
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
    fn reset_for_program(&mut self, program: &Program) {
        self.stack.clear();
        self.color.reset(program.e_nodes.len(), Color::White);
    }

    /// Build the per-run schedule into `schedule` from the installed program and the run's
    /// `seeds` (the roots to walk back from). Exact execution-node seeds are roots
    /// directly. Errors on a dependency cycle or a node/event seed absent from the program.
    ///
    /// The [`Scheduled`] handed back is the only way to reach
    /// [`resolve`](Scheduled::resolve), and it borrows `schedule` for as long as it
    /// lives — so nothing can read a half-filled buffer, and the program the columns
    /// are aligned to travels with them rather than being passed again downstream.
    pub(crate) fn plan<'a>(
        &mut self,
        program: &'a Program,
        seeds: &RunSeeds,
        schedule: &'a mut RunSchedule,
    ) -> Result<Scheduled<'a>> {
        schedule.reset_for_program(program);
        self.reset_for_program(program);

        // Collect the walk roots straight into `schedule.roots` — they seed the
        // backward walk below and the cache-aware reverse sweep.
        schedule.collect_roots(program, seeds)?;

        self.walk_backward_collect_order(program, schedule)?;
        schedule.validate_debug(program);
        Ok(Scheduled::new(program, schedule))
    }

    /// Backward post-order DFS from the roots: builds `process_order` (deps before
    /// consumers), detects cycles, and — folded in here rather than a separate forward
    /// pass — resolves each node's structural
    /// [`NodeState`].
    /// The state is set in the `Done` arm, i.e. in post-order, so every Bind dep is
    /// already `Black` with its own state set when a consumer reads it (what the old
    /// separate `resolve_verdicts` pass asserted, now structural).
    fn walk_backward_collect_order(
        &mut self,
        program: &Program,
        schedule: &mut RunSchedule,
    ) -> Result<()> {
        for node_idx in schedule.roots.iter() {
            self.stack.push(Visit::Discover(node_idx));
        }

        while let Some(visit) = self.stack.pop() {
            let node_idx = match visit {
                Visit::Discover(node_idx) => node_idx,
                Visit::Done(node_idx) => {
                    debug_assert_eq!(self.color[node_idx], Color::Gray);
                    self.color[node_idx] = Color::Black;
                    schedule.process_order.push(node_idx);
                    // Runnable unless a required input is unbound or fed by a
                    // non-runnable producer. Post-order ⇒ deps already verdicted, so
                    // `input_missing` reads settled values. Whether the node's output is
                    // reused from cache is decided at execution, not here.
                    let missing = program.inputs[program[node_idx].inputs]
                        .iter()
                        .any(|e_input| schedule.input_missing(e_input));
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

            match self.color[node_idx] {
                Color::Gray => {
                    return Err(Error::CycleDetected {
                        e_node_id: program.e_node_ids[node_idx],
                    });
                }
                Color::Black => continue,
                Color::White => {}
            }

            let e_node = &program[node_idx];
            // Disabled nodes block dependency traversal, but an explicit node
            // seed is recorded before this walk and overrides disable for this run.
            if e_node.disabled && !schedule.seeded.contains(node_idx) {
                self.color[node_idx] = Color::Black;
                schedule.states[node_idx] = NodeState::Disabled;
                continue;
            }

            self.color[node_idx] = Color::Gray;
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
