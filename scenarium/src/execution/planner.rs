//! Scheduling: turns an [`ExecutionProgram`](crate::execution::program::ExecutionProgram)
//! plus the current runtime cache state into an
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan). Two backward DFS passes
//! (order + cycle detection, then prune to the needed set) bracket a forward
//! propagation that resolves each node's cached/wants-execute state. The
//! `Planner` owns the reusable DFS scratch so a repeated plan allocates nothing.

use crate::execution::cache::Cache;
use crate::execution::plan::{ExecutionPlan, NodeVerdict, input_missing};
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeColumn, NodeIdx};
use crate::execution::{Error, Result, RunSeeds, validate};

/// DFS coloring for the two backward passes. White = unvisited, Gray = on
/// stack (Done pushed, children pending), Black = children done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

/// Why a node sits on the DFS stack. `Root`/`Visit` both mean "discover this node"
/// (a walk root vs. a producer reached from a consumer); they differ only as
/// documentation. `Done` is the post-order marker pushed under a node's children.
/// Output-usage is counted at push time (per consumer edge), so the producer-visit
/// carries no port — both backward passes share this shape.
#[derive(Debug)]
enum VisitCause {
    Root,
    Visit,
    Done,
}

#[derive(Debug)]
struct Visit {
    e_node_idx: NodeIdx,
    cause: VisitCause,
}

/// Reusable per-run scheduling scratch, kept across runs so a repeated plan on
/// an unchanged graph does no scheduling allocations.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    /// DFS coloring, reused across *both* backward passes (reset between).
    color: NodeColumn<Color>,
    /// DFS work stack.
    stack: Vec<Visit>,
    /// Root-membership marker column (dedup without hashing).
    is_root: NodeColumn<bool>,
    /// Deduped indices of the nodes the backward walks start from — terminals,
    /// event subscribers, and event-trigger owners. *Not* only `terminal` nodes.
    roots: Vec<NodeIdx>,
}

impl Planner {
    /// Build the per-run schedule into `plan` from the program and the cache
    /// state (which nodes hold a valid cached output). Errors only on a
    /// dependency cycle.
    pub(crate) fn plan(
        &mut self,
        program: &ExecutionProgram,
        cache: &Cache,
        seeds: &RunSeeds,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.reset(program.e_nodes.len(), program.n_outputs);

        self.collect_roots(program, seeds);

        let result = self.walk_backward_collect_order(program, cache, plan);
        if result.is_ok() {
            self.walk_backward_collect_execute_order(program, plan);
            validate::schedule(program, plan);
        }
        result
    }

    /// Mark `idx` as a walk root, deduping via the marker column.
    fn mark_root(&mut self, idx: NodeIdx) {
        if !self.is_root[idx] {
            self.is_root[idx] = true;
            self.roots.push(idx);
        }
    }

    fn collect_roots(&mut self, program: &ExecutionProgram, seeds: &RunSeeds) {
        self.is_root.reset(program.e_nodes.len(), false);
        self.roots.clear();

        // Add event subscribers
        for event in &seeds.events {
            let e_node = program.e_nodes.by_key(&event.node_id).unwrap();
            let subs = program.events[e_node.events.range()][event.event_idx]
                .subscribers
                .clone();
            for sub in subs {
                self.mark_root(sub);
            }
        }

        // Add terminal nodes
        if seeds.terminals {
            for (idx, e) in program.e_nodes.iter().enumerate() {
                if e.terminal {
                    self.mark_root(idx.into());
                }
            }
        }

        // Add nodes with event triggers
        if seeds.event_triggers {
            for (idx, e) in program.e_nodes.iter().enumerate() {
                if program.events[e.events.range()]
                    .iter()
                    .any(|ev| !ev.subscribers.is_empty())
                {
                    self.mark_root(idx.into());
                }
            }
        }
    }

    /// Backward post-order DFS from the roots: builds `process_order` (deps before
    /// consumers), counts per-output usage, detects cycles, and — folded in here
    /// rather than a separate forward pass — resolves each node's [`NodeVerdict`].
    /// The verdict is set in the `Done` arm, i.e. in post-order, so every Bind dep is
    /// already `Black` with its own verdict set when a consumer reads it (what the old
    /// separate `resolve_verdicts` pass asserted, now structural).
    fn walk_backward_collect_order(
        &mut self,
        program: &ExecutionProgram,
        cache: &Cache,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.process_order.clear();
        self.stack.clear();

        self.color.reset(program.e_nodes.len(), Color::White);

        for e_node_idx in self.roots.iter().copied() {
            self.stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Root,
            });
        }

        while let Some(visit) = self.stack.pop() {
            match visit.cause {
                VisitCause::Root | VisitCause::Visit => {}
                VisitCause::Done => {
                    let idx = visit.e_node_idx;
                    assert_eq!(self.color[idx], Color::Gray);
                    self.color[idx] = Color::Black;
                    plan.process_order.push(idx);
                    // Cached if the output is available (resident or a flagged disk
                    // blob); otherwise runnable unless a required input is unbound or
                    // fed by a non-runnable producer. Post-order ⇒ deps already
                    // verdicted, so `input_missing` reads settled values.
                    plan.verdicts[idx] = if cache.is_available(idx) {
                        NodeVerdict::Cached
                    } else {
                        let inputs = program.e_nodes[idx].inputs;
                        let missing = program.inputs[inputs.range()]
                            .iter()
                            .any(|e_input| input_missing(e_input, &plan.verdicts));
                        if missing {
                            NodeVerdict::MissingInputs
                        } else {
                            NodeVerdict::Execute
                        }
                    };
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
                    // Count this consumer's read of the producer's port (drives the
                    // executor's per-output Skip/Needed); once per consumer edge,
                    // counted at push so the visit cause needs no payload.
                    let outputs = program.e_nodes[addr.target_idx].outputs;
                    plan.output_usage[outputs.start as usize + addr.port_idx] += 1;
                    self.stack.push(Visit {
                        e_node_idx: addr.target_idx,
                        cause: VisitCause::Visit,
                    });
                }
            }
        }

        Ok(())
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

        self.color.reset(program.e_nodes.len(), Color::White);

        for e_node_idx in self.roots.iter().copied() {
            self.stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Root,
            });
        }

        while let Some(visit) = self.stack.pop() {
            let idx = visit.e_node_idx;

            match visit.cause {
                VisitCause::Root | VisitCause::Visit => {}
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

            if !plan.verdicts[idx].wants_execute() {
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
                    && plan.verdicts[addr.target_idx].wants_execute()
                {
                    self.stack.push(Visit {
                        e_node_idx: addr.target_idx,
                        cause: VisitCause::Visit,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::cache::ValueCache;
    use crate::execution::digest::Digest;
    use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
    use crate::graph::NodeId;
    use crate::prelude::FuncId;
    use common::Span;

    /// Hand-built program for planner tests. Node `idx` gets id `from_u128(idx+1)`.
    /// Inputs are `(required, binding)`.
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
        ) -> NodeIdx {
            let inputs_start = self.program.inputs.len() as u32;
            for (required, binding) in inputs {
                self.program.inputs.push(ExecutionInput {
                    required: *required,
                    binding: binding.clone(),
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
            idx.into()
        }
    }

    fn bind(idx: NodeIdx, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(ExecutionPortAddress {
            target_idx: idx,
            port_idx: port,
        })
    }

    /// Plan `terminals` over `fix`, with the slots at `cached` marked as RAM hits.
    fn plan_with_cached(fix: &Fix, cached: &[NodeIdx]) -> ExecutionPlan {
        let mut cache = Cache::default();
        cache.reconcile(&fix.program.e_nodes);
        for &idx in cached {
            let digest = Digest([idx.idx() as u8 + 1; 32]);
            cache.slots[idx].current_digest = Some(digest);
            cache.slots[idx].value = ValueCache::Resident {
                values: Vec::new(),
                produced_under: Some(digest),
            };
        }
        let mut planner = Planner::default();
        let mut plan = ExecutionPlan::default();
        let seeds = RunSeeds {
            terminals: true,
            ..Default::default()
        };
        planner
            .plan(&fix.program, &cache, &seeds, &mut plan)
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
            assert!(p.verdicts[idx].wants_execute());
            assert!(!p.verdicts[idx].is_cached());
            assert!(!p.verdicts[idx].missing_required_inputs());
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
        assert!(p.verdicts[b].is_cached());
        assert!(p.verdicts[a].wants_execute(), "A wants to run…");
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
                p.verdicts[idx].missing_required_inputs(),
                "node {idx:?} missing"
            );
            assert!(
                !p.verdicts[idx].wants_execute(),
                "node {idx:?} not runnable"
            );
        }
        assert!(p.execute_order.is_empty());
    }

    #[test]
    fn optional_unbound_input_does_not_block() {
        // An *optional* unbound input is fine — the node still runs.
        let mut f = Fix::default();
        let a = f.node(true, &[(false, ExecutionBinding::None)], 1);

        let p = plan(&f);
        assert!(!p.verdicts[a].missing_required_inputs());
        assert!(p.verdicts[a].wants_execute());
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
        f.node(true, &[(false, bind(NodeIdx(1), 0))], 1); // A (idx 0) binds B
        f.node(false, &[(false, bind(NodeIdx(0), 0))], 1); // B (idx 1) binds A

        let mut cache = Cache::default();
        cache.reconcile(&f.program.e_nodes);
        let mut planner = Planner::default();
        let mut plan = ExecutionPlan::default();
        let seeds = RunSeeds {
            terminals: true,
            ..Default::default()
        };
        let result = planner.plan(&f.program, &cache, &seeds, &mut plan);
        assert!(matches!(result, Err(Error::CycleDetected { .. })));
    }
}
