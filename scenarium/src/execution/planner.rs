//! Scheduling: turns an [`ExecutionProgram`](crate::execution::program::ExecutionProgram)
//! plus the current runtime cache state into an
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan). Two backward DFS passes
//! (order + cycle detection, then prune to the needed set) bracket a forward
//! propagation that resolves each node's cached/wants-execute state. The
//! `Planner` owns the reusable DFS scratch so a repeated plan allocates nothing.

use common::is_debug;
use hashbrown::HashSet;

use crate::worker::EventRef;

use crate::execution::executor::Executor;
use crate::execution::plan::{ExecutionPlan, InputFlags};
use crate::execution::program::{ExecutionBehavior, ExecutionBinding, ExecutionProgram};
use crate::execution::{Error, Result};

/// DFS coloring for the two backward passes. White = unvisited, Gray = on
/// stack (Done pushed, children pending), Black = children done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

#[derive(Debug)]
enum VisitCause {
    Terminal,
    OutputRequest { output_idx: usize },
    Done,
}

#[derive(Debug)]
struct Visit {
    e_node_idx: usize,
    cause: VisitCause,
}

/// Reusable per-run scheduling scratch, kept across runs so a repeated plan on
/// an unchanged graph does no scheduling allocations.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    /// DFS coloring, reused across *both* backward passes (reset between).
    color: Vec<Color>,
    /// DFS work stack.
    stack: Vec<Visit>,
    /// Terminal-membership marker column (dedup without hashing).
    is_terminal: Vec<bool>,
    /// Deduped terminal node indices that seed the backward walks.
    terminal_seeds: Vec<usize>,
}

impl Planner {
    /// Build the per-run schedule into `plan` from the program and the
    /// executor's cache state (which nodes hold cached outputs). Errors only on
    /// a dependency cycle.
    pub(crate) fn plan(
        &mut self,
        program: &ExecutionProgram,
        executor: &Executor,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.reset(
            program.e_nodes.len(),
            program.inputs.len(),
            program.n_outputs,
        );

        self.collect_terminal_nodes(program, terminals, event_triggers, events);

        let result = self.walk_backward_collect_order(program, plan);
        if result.is_ok() {
            self.propagate_input_state_forward(program, executor, plan);
            self.walk_backward_collect_execute_order(program, plan);
            self.validate_for_execution(program, plan);
        }
        result
    }

    /// Mark `idx` as a terminal seed, deduping via the marker column.
    fn mark_terminal(&mut self, idx: usize) {
        if !self.is_terminal[idx] {
            self.is_terminal[idx] = true;
            self.terminal_seeds.push(idx);
        }
    }

    fn collect_terminal_nodes(
        &mut self,
        program: &ExecutionProgram,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) {
        self.is_terminal.clear();
        self.is_terminal.resize(program.e_nodes.len(), false);
        self.terminal_seeds.clear();

        // Add event subscribers
        for event in events {
            let e_node = program.e_nodes.by_key(&event.node_id).unwrap();
            let subs = program.events[e_node.events.range()][event.event_idx]
                .subscribers
                .clone();
            for sub in &subs {
                let idx = program.e_nodes.index_of_key(sub).unwrap();
                self.mark_terminal(idx);
            }
        }

        // Add terminal nodes
        if terminals {
            for (idx, e) in program.e_nodes.iter().enumerate() {
                if e.terminal {
                    self.mark_terminal(idx);
                }
            }
        }

        // Add nodes with event triggers
        if event_triggers {
            for (idx, e) in program.e_nodes.iter().enumerate() {
                if program.events[e.events.range()]
                    .iter()
                    .any(|ev| !ev.subscribers.is_empty())
                {
                    self.mark_terminal(idx);
                }
            }
        }
    }

    fn walk_backward_collect_order(
        &mut self,
        program: &ExecutionProgram,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.process_order.clear();
        self.stack.clear();

        self.color.clear();
        self.color.resize(program.e_nodes.len(), Color::White);

        for e_node_idx in self.terminal_seeds.iter().copied() {
            self.stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = self.stack.pop() {
            match visit.cause {
                VisitCause::Terminal => {}
                VisitCause::OutputRequest { output_idx } => {
                    let span = program.e_nodes[visit.e_node_idx].outputs;
                    plan.output_usage[span.start as usize + output_idx] += 1;
                }
                VisitCause::Done => {
                    assert_eq!(self.color[visit.e_node_idx], Color::Gray);
                    self.color[visit.e_node_idx] = Color::Black;
                    plan.process_order.push(visit.e_node_idx);
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
                    self.stack.push(Visit {
                        e_node_idx: addr.target_idx,
                        cause: VisitCause::OutputRequest {
                            output_idx: addr.port_idx,
                        },
                    });
                }
            }
        }

        Ok(())
    }

    fn propagate_input_state_forward(
        &self,
        program: &ExecutionProgram,
        executor: &Executor,
        plan: &mut ExecutionPlan,
    ) {
        // Debug-only: verify every Bind dep was already processed in this
        // forward pass. Guaranteed by process_order being post-order DFS
        // (deps before consumers), but worth checking — if this flips, the
        // forward pass is reading a stale `wants_execute`/`missing_required`.
        let mut processed = if is_debug() {
            vec![false; program.e_nodes.len()]
        } else {
            Vec::new()
        };

        for order_idx in 0..plan.process_order.len() {
            let e_node_idx = plan.process_order[order_idx];
            let inputs_span = program.e_nodes[e_node_idx].inputs;

            let mut inputs_updated = false;
            let mut bindings_changed = false;
            let mut missing_required = false;

            for pool_idx in inputs_span.range() {
                let e_input = &program.inputs[pool_idx];
                let binding_changed = executor.input_dirty[pool_idx];
                let (dep_wants_execute, missing) = match &e_input.binding {
                    // Unbound is "missing" only if the input is required — an
                    // optional input left unbound is a deliberate no-value.
                    ExecutionBinding::None => (false, e_input.required),
                    ExecutionBinding::Const(_) => (false, false),
                    // A *wired* input whose producer can't run (missing its own
                    // required inputs) has no value to deliver, so the consumer
                    // is missing too — regardless of whether the input is
                    // optional. (Optional only excuses an *unbound* input, not a
                    // binding to a broken upstream.)
                    ExecutionBinding::Bind(addr) => {
                        let target_idx = addr.target_idx;
                        assert!(addr.port_idx < program.e_nodes[target_idx].outputs.len as usize);
                        if is_debug() {
                            assert!(processed[target_idx], "forward pass: dep not yet processed");
                        }
                        let dep = plan.node_flags[target_idx];
                        (dep.wants_execute, dep.missing_required_inputs)
                    }
                };

                plan.input_flags[pool_idx] = InputFlags {
                    dependency_wants_execute: dep_wants_execute,
                    missing,
                };
                inputs_updated |= binding_changed || dep_wants_execute;
                bindings_changed |= binding_changed;
                missing_required |= missing;
            }

            let behavior = program.e_nodes[e_node_idx].behavior;
            let has_outputs = executor.slots[e_node_idx].output_values.is_some();
            let flags = &mut plan.node_flags[e_node_idx];
            flags.inputs_updated = inputs_updated;
            flags.missing_required_inputs = missing_required;

            if missing_required {
                flags.wants_execute = false;
                flags.cached = false;
            } else if bindings_changed {
                flags.wants_execute = true;
                flags.cached = false;
            } else {
                flags.cached = match behavior {
                    ExecutionBehavior::Impure => false,
                    ExecutionBehavior::Pure => has_outputs && !inputs_updated,
                    ExecutionBehavior::Once => has_outputs,
                };
                flags.wants_execute = !flags.cached;
            }

            if is_debug() {
                processed[e_node_idx] = true;
            }
        }
    }

    // Prunes `process_order` to only nodes whose output is actually read by an
    // executing consumer this run. A filter over `wants_execute` is not
    // equivalent: a Pure/Impure node can have `wants_execute = true` while its
    // sole consumer is Once-cached and won't read it — the forward pass can't
    // see that because "needed by consumer" is a backward fact. See
    // `once_node_toggle_refreshes_upstream` in tests.rs for the case this pass
    // exists to handle.
    fn walk_backward_collect_execute_order(
        &mut self,
        program: &ExecutionProgram,
        plan: &mut ExecutionPlan,
    ) {
        plan.execute_order.clear();
        self.stack.clear();

        self.color.clear();
        self.color.resize(program.e_nodes.len(), Color::White);

        for e_node_idx in self.terminal_seeds.iter().copied() {
            self.stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = self.stack.pop() {
            let idx = visit.e_node_idx;

            match visit.cause {
                VisitCause::Terminal | VisitCause::OutputRequest { .. } => {}
                VisitCause::Done => {
                    assert_eq!(self.color[idx], Color::Gray);
                    plan.execute_order.push(idx);
                    self.color[idx] = Color::Black;
                    continue;
                }
            }

            match self.color[idx] {
                Color::White => {}
                Color::Black => continue,
                // Pass 1 would have rejected any cycle; a Gray revisit in pass 2
                // means our DFS invariant is broken.
                Color::Gray => unreachable!("cycle should be detected in pass 1"),
            }

            if !plan.node_flags[idx].wants_execute {
                self.color[idx] = Color::Black;
                continue;
            }

            self.color[idx] = Color::Gray;
            self.stack.push(Visit {
                e_node_idx: idx,
                cause: VisitCause::Done,
            });

            let span = program.e_nodes[idx].inputs;
            for (pool_idx, e_input) in program.inputs[span.range()].iter().enumerate() {
                if plan.input_flags[span.start as usize + pool_idx].dependency_wants_execute
                    && let Some(addr) = e_input.binding.as_bind()
                {
                    self.stack.push(Visit {
                        e_node_idx: addr.target_idx,
                        cause: VisitCause::OutputRequest {
                            output_idx: addr.port_idx,
                        },
                    });
                }
            }
        }
    }

    fn validate_for_execution(&self, program: &ExecutionProgram, plan: &ExecutionPlan) {
        if !is_debug() {
            return;
        }

        assert!(plan.process_order.len() <= program.e_nodes.len());

        // `process_order` is a post-order DFS: unique, and every Bind dep
        // appears before its consumer.
        let mut seen_in_order = HashSet::with_capacity(program.e_nodes.len());
        for &idx in &plan.process_order {
            assert!(idx < program.e_nodes.len());
            let span = program.e_nodes[idx].inputs;
            for input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    assert!(addr.target_idx < program.e_nodes.len());
                    assert!(seen_in_order.contains(&addr.target_idx));
                }
            }
            assert!(seen_in_order.insert(idx));
        }

        for (idx, e_node) in program.e_nodes.iter().enumerate() {
            let flags = plan.node_flags[idx];
            if flags.missing_required_inputs {
                assert!(!flags.wants_execute);
            }

            for e_input in &program.inputs[e_node.inputs.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    assert!(addr.target_idx < program.e_nodes.len());
                    assert!(addr.port_idx < program.e_nodes[addr.target_idx].outputs.len as usize);
                }
            }
        }

        assert!(plan.execute_order.len() <= plan.process_order.len());

        let mut pending: HashSet<usize> = plan.execute_order.iter().copied().collect();
        assert_eq!(pending.len(), plan.execute_order.len());

        for &idx in &plan.execute_order {
            assert!(idx < program.e_nodes.len());
            pending.remove(&idx);

            let e_node = &program.e_nodes[idx];
            let flags = plan.node_flags[idx];
            assert!(flags.wants_execute);
            assert!(!flags.missing_required_inputs);

            for e_input in &program.inputs[e_node.inputs.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    assert!(!pending.contains(&addr.target_idx));
                }
            }
        }
    }
}
