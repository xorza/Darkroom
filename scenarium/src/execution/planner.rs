//! Scheduling: turns an [`ExecutionProgram`](crate::execution::program::ExecutionProgram)
//! plus the current runtime cache state into an
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan). Two backward DFS passes
//! (order + cycle detection, then prune to the needed set) bracket a forward
//! propagation that resolves each node's cached/wants-execute state. The
//! `Planner` owns the reusable DFS scratch so a repeated plan allocates nothing.

use common::is_debug;

use crate::execution::cache::Cache;
use crate::execution::event::EventRef;
use crate::execution::plan::{ExecutionPlan, NodeFlags};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::execution::{Error, Result, validate};

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
    /// Build the per-run schedule into `plan` from the program and the cache
    /// state (which nodes hold a valid cached output). Errors only on a
    /// dependency cycle.
    pub(crate) fn plan(
        &mut self,
        program: &ExecutionProgram,
        cache: &Cache,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.reset(program.e_nodes.len(), program.n_outputs);

        self.collect_terminal_nodes(program, terminals, event_triggers, events);

        let result = self.walk_backward_collect_order(program, plan);
        if result.is_ok() {
            self.resolve_node_flags(program, cache, plan);
            self.walk_backward_collect_execute_order(program, plan);
            validate::schedule(program, plan);
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

    fn resolve_node_flags(
        &self,
        program: &ExecutionProgram,
        cache: &Cache,
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

            // Cache hit: the slot holds a value produced under this node's current
            // digest (a disk-cached value was hydrated into the slot at update, so
            // this is a RAM check either way). `None` (impure cone) never matches.
            let cached = cache.is_hit(e_node_idx);

            if cached {
                plan.node_flags[e_node_idx] = NodeFlags {
                    cached: true,
                    wants_execute: false,
                    missing_required_inputs: false,
                };
                if is_debug() {
                    processed[e_node_idx] = true;
                }
                continue;
            }

            // Not cached: a node is runnable unless a required input is unbound,
            // or wired to a producer that itself can't run (`missing` propagates
            // only through non-runnable producers — a cached or executing one
            // delivers a value, optional or not). The per-input verdict isn't
            // stored — `collect_execution_stats` recomputes it for the rare
            // missing node when reporting which ports are unsatisfied.
            let inputs_span = program.e_nodes[e_node_idx].inputs;
            let mut missing_required = false;
            for pool_idx in inputs_span.range() {
                let e_input = &program.inputs[pool_idx];
                let missing = match &e_input.binding {
                    ExecutionBinding::None => e_input.required,
                    ExecutionBinding::Const(_) => false,
                    ExecutionBinding::Bind(addr) => {
                        assert!(
                            addr.port_idx < program.e_nodes[addr.target_idx].outputs.len as usize
                        );
                        if is_debug() {
                            assert!(
                                processed[addr.target_idx],
                                "forward pass: dep not yet processed"
                            );
                        }
                        plan.node_flags[addr.target_idx].missing_required_inputs
                    }
                };
                missing_required |= missing;
            }

            plan.node_flags[e_node_idx] = NodeFlags {
                cached: false,
                wants_execute: !missing_required,
                missing_required_inputs: missing_required,
            };
            if is_debug() {
                processed[e_node_idx] = true;
            }
        }
    }

    // Prunes `process_order` to only nodes whose output is actually read by an
    // executing consumer this run. A filter over `wants_execute` is not
    // equivalent: a node can have `wants_execute = true` while its sole consumer
    // is cached and won't read it — the forward pass can't see that because
    // "needed by consumer" is a backward fact.
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
            for e_input in &program.inputs[span.range()] {
                // Recurse only into a producer that will itself run; a cached
                // producer's value is already available, so its cone is pruned.
                if let Some(addr) = e_input.binding.as_bind()
                    && plan.node_flags[addr.target_idx].wants_execute
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataType;
    use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
    use crate::graph::NodeId;
    use crate::prelude::FuncId;
    use common::Span;

    /// Hand-built program for planner tests. Node `idx` gets id `from_u128(idx+1)`,
    /// so `bind`'s `target_idx`/`target_id` line up. Inputs are `(required, binding)`.
    #[derive(Default)]
    struct Fix {
        program: ExecutionProgram,
    }

    impl Fix {
        fn node(
            &mut self,
            terminal: bool,
            inputs: &[(bool, ExecutionBinding)],
            outputs: u32,
        ) -> usize {
            let inputs_start = self.program.inputs.len() as u32;
            for (required, binding) in inputs {
                self.program.inputs.push(ExecutionInput {
                    required: *required,
                    binding: binding.clone(),
                    data_type: DataType::Null,
                });
            }
            let outputs_start = self.program.n_outputs as u32;
            self.program.n_outputs += outputs as usize;
            let idx = self.program.e_nodes.len();
            self.program.e_nodes.add(ExecutionNode {
                id: NodeId::from_u128(idx as u128 + 1),
                inited: true,
                terminal,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs: Span::new(inputs_start, inputs.len() as u32),
                outputs: Span::new(outputs_start, outputs),
                ..Default::default()
            });
            idx
        }
    }

    fn bind(idx: usize, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(ExecutionPortAddress {
            target_id: NodeId::from_u128(idx as u128 + 1),
            target_idx: idx,
            port_idx: port,
        })
    }

    /// Plan `terminals` over `fix`, with the slots at `cached` marked as RAM hits.
    fn plan_with_cached(fix: &Fix, cached: &[usize]) -> ExecutionPlan {
        let mut cache = Cache::default();
        cache.reconcile(&fix.program.e_nodes);
        for &idx in cached {
            let digest = [idx as u8 + 1; 32];
            cache.slots[idx].current_digest = Some(digest);
            cache.slots[idx].output_values = Some(Vec::new());
            cache.slots[idx].output_digest = Some(digest);
        }
        let mut planner = Planner::default();
        let mut plan = ExecutionPlan::default();
        planner
            .plan(&fix.program, &cache, true, false, &[], &mut plan)
            .expect("no cycle");
        plan
    }

    fn plan(fix: &Fix) -> ExecutionPlan {
        plan_with_cached(fix, &[])
    }

    #[test]
    fn chain_orders_deps_before_consumers_and_schedules_all() {
        // A → B → C (C terminal), nothing cached.
        let mut f = Fix::default();
        let a = f.node(false, &[], 1);
        let b = f.node(false, &[(false, bind(a, 0))], 1);
        let c = f.node(true, &[(false, bind(b, 0))], 1);

        let p = plan(&f);
        assert_eq!(p.process_order, vec![a, b, c], "post-order: deps first");
        assert_eq!(p.execute_order, vec![a, b, c]);
        for idx in [a, b, c] {
            assert!(p.node_flags[idx].wants_execute);
            assert!(!p.node_flags[idx].cached);
            assert!(!p.node_flags[idx].missing_required_inputs);
        }
    }

    #[test]
    fn cached_consumer_prunes_its_producer_from_execute_order() {
        // A → B (B terminal). B is cached. A "wants" to run, but its only consumer
        // won't read it, so A must not be scheduled — the case a plain
        // filter(wants_execute) would get wrong.
        let mut f = Fix::default();
        let a = f.node(false, &[], 1);
        let b = f.node(true, &[(false, bind(a, 0))], 1);

        let p = plan_with_cached(&f, &[b]);
        assert!(p.node_flags[b].cached);
        assert!(p.node_flags[a].wants_execute, "A wants to run…");
        assert!(
            p.execute_order.is_empty(),
            "…but isn't scheduled: its sole consumer is cached"
        );
    }

    #[test]
    fn missing_required_input_blocks_node_and_dependents() {
        // A has a required *unbound* input ⇒ missing; B binds A ⇒ inherits missing.
        let mut f = Fix::default();
        let a = f.node(false, &[(true, ExecutionBinding::None)], 1);
        let b = f.node(true, &[(false, bind(a, 0))], 1);

        let p = plan(&f);
        for idx in [a, b] {
            assert!(
                p.node_flags[idx].missing_required_inputs,
                "node {idx} missing"
            );
            assert!(!p.node_flags[idx].wants_execute, "node {idx} not runnable");
        }
        assert!(p.execute_order.is_empty());
    }

    #[test]
    fn optional_unbound_input_does_not_block() {
        // An *optional* unbound input is fine — the node still runs.
        let mut f = Fix::default();
        let a = f.node(true, &[(false, ExecutionBinding::None)], 1);

        let p = plan(&f);
        assert!(!p.node_flags[a].missing_required_inputs);
        assert!(p.node_flags[a].wants_execute);
        assert_eq!(p.execute_order, vec![a]);
    }

    #[test]
    fn fan_out_counts_each_executing_consumer() {
        // A feeds both B and C (both terminal) ⇒ A's output is needed twice.
        let mut f = Fix::default();
        let a = f.node(false, &[], 1);
        f.node(true, &[(false, bind(a, 0))], 1);
        f.node(true, &[(false, bind(a, 0))], 1);

        let p = plan(&f);
        assert_eq!(p.output_usage[0], 2, "A.0 read by two consumers");
    }

    #[test]
    fn dependency_cycle_is_rejected() {
        // A binds B, B binds A (A terminal) — the planner must error, not loop.
        let mut f = Fix::default();
        f.node(true, &[(false, bind(1, 0))], 1); // A (idx 0) binds B
        f.node(false, &[(false, bind(0, 0))], 1); // B (idx 1) binds A

        let mut cache = Cache::default();
        cache.reconcile(&f.program.e_nodes);
        let mut planner = Planner::default();
        let mut plan = ExecutionPlan::default();
        let result = planner.plan(&f.program, &cache, true, false, &[], &mut plan);
        assert!(matches!(result, Err(Error::CycleDetected { .. })));
    }
}
