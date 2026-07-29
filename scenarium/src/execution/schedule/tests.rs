//! Both passes over the [`RunSchedule`](super::RunSchedule), each with the fixture
//! its own pass needs: a bare program for the structural walk, and a program plus a
//! primed cache for the sweep that reads one.

mod planning {
    use crate::execution::error::Error;
    use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId};
    use crate::execution::program::index::{NodeIdx, OutputAddr, OutputIdx};
    use crate::execution::program::{
        ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionOutput, Program,
    };
    use crate::execution::schedule::planner::Planner;
    use crate::execution::schedule::{NodeState, ResolvedOutputs, RunSchedule};
    use crate::execution::seeds::RunSeeds;
    use crate::node::definition::FuncId;
    use crate::node::lambda::OutputDemand;

    /// Hand-built program for planner tests — scheduling is structural, so it
    /// needs no compile artifact and no authoring attribution. Inputs are
    /// `(required, binding)`.
    #[derive(Default)]
    struct Fix {
        program: Program,
    }

    impl Fix {
        fn node(
            &mut self,
            sink: bool,
            inputs: &[(bool, ExecutionBinding)],
            outputs: u32,
        ) -> ExecutionNodeId {
            let program = &mut self.program;
            let inputs = program
                .inputs
                .append(inputs.iter().map(|(required, binding)| ExecutionInput {
                    required: *required,
                    stamps_fs_path: false,
                    binding: binding.clone(),
                }));
            let outputs = program
                .outputs
                .append((0..outputs).map(|_| ExecutionOutput::default()));
            let idx = program.e_nodes.len();
            let id = ExecutionNodeId::from_u128(idx as u128 + 1);
            program.push(
                id,
                ExecutionNode {
                    sink,
                    func_id: FuncId::from_u128(idx as u128 + 1),
                    inputs,
                    outputs,
                    ..Default::default()
                },
            );
            id
        }
    }

    /// The fixture's id ↔ index invariant: ids are assigned `from_u128(idx + 1)`
    /// in push order, so a node's dense index is recoverable from its id.
    fn nx(e_node_id: ExecutionNodeId) -> NodeIdx {
        NodeIdx(e_node_id.as_uuid().as_u128() as u32 - 1)
    }

    fn bind(e_node_id: ExecutionNodeId, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(OutputAddr {
            node_idx: nx(e_node_id),
            port_idx: port as u32,
        })
    }

    /// Plan `sinks` over `fix`. Purely structural — no cache state; the executor
    /// decides cached-vs-recompute at run time.
    fn plan(fix: &Fix) -> RunSchedule {
        let mut planner = Planner::default();
        let mut plan = RunSchedule::default();
        let seeds = RunSeeds {
            sinks: true,
            ..Default::default()
        };
        planner
            .plan(&fix.program, &seeds, &mut plan)
            .expect("no cycle");
        plan
    }

    #[test]
    #[cfg(debug_assertions)]
    fn reader_overflow_trips_the_debug_invariant() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let mut outputs = ResolvedOutputs::default();
        outputs.reset(1);
        outputs.readers[OutputIdx(0)] = u32::MAX;

        assert!(
            catch_unwind(AssertUnwindSafe(|| outputs.add_reader(OutputIdx(0)))).is_err(),
            "a graph cannot have more readers than the counter represents"
        );
    }

    /// The planner opens the output half too: a schedule it just filled carries a
    /// zeroed demand and reader column spanning the program's output pool, so no
    /// previous run's counts can survive into one the sweep has not touched.
    #[test]
    fn planning_resets_the_output_half_to_the_programs_pool() {
        let mut f = Fix::default();
        let a = f.node(false, &[], 2);
        f.node(true, &[(false, bind(a, 0))], 1);

        let mut planner = Planner::default();
        let mut schedule = RunSchedule::default();
        // A stale resolution from an earlier, differently shaped run.
        schedule.outputs.reset(1);
        schedule.outputs.add_reader(OutputIdx(0));

        planner
            .plan(
                &f.program,
                &RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                &mut schedule,
            )
            .expect("no cycle");

        assert_eq!(
            schedule.outputs.demand.len(),
            3,
            "two outputs plus the sink's"
        );
        assert_eq!(schedule.outputs.readers.len(), 3);
        assert!(
            schedule
                .outputs
                .demand
                .iter()
                .all(|demand| *demand == OutputDemand::Skip)
        );
        assert!(schedule.outputs.readers.iter().all(|readers| *readers == 0));
    }

    #[test]
    fn chain_orders_deps_before_consumers_and_schedules_all() {
        // A → B → C (C sink). Every reachable node is scheduled — the planner is
        // structural, so nothing is pruned as "cached" here (that's the executor's call).
        let mut f = Fix::default();
        let a = f.node(false, &[], 1);
        let b = f.node(false, &[(false, bind(a, 0))], 1);
        let c = f.node(true, &[(false, bind(b, 0))], 1);

        let mut p = plan(&f);
        p.validate(&f.program).unwrap();
        assert_eq!(p.process_order, [a, b, c].map(nx), "post-order: deps first");
        for idx in [a, b, c] {
            assert!(p.states[nx(idx)].is_runnable());
            assert!(!p.states[nx(idx)].missing_required_inputs());
        }

        p.process_order.swap(0, 1);
        assert_eq!(
            p.validate(&f.program).unwrap_err().to_string(),
            format!("execution node {b:?} appears before dependency {a:?}")
        );
        p.states[nx(a)] = NodeState::Disabled;
        assert_eq!(
            p.validate(&f.program).unwrap_err().to_string(),
            format!("execution node {b:?} appears before dependency {a:?}"),
            "a disabled verdict cannot hide an enabled dependency"
        );
        p.process_order.swap(0, 1);
        // Back to what the planner actually wrote — not `default()`, which is the
        // `Unvisited` fill and would leave a scheduled node undecided.
        p.states[nx(a)] = NodeState::Cut;
        p.validate(&f.program).expect("restored to a valid plan");

        // Scheduling a node and deciding its state are one act, so a state may not
        // be decided for a node the schedule left out.
        let dropped = p
            .process_order
            .pop()
            .expect("the chain scheduled three nodes");
        assert_eq!(dropped, nx(c));
        assert_eq!(
            p.validate(&f.program).unwrap_err().to_string(),
            format!("unscheduled node {c:?} was decided Cut")
        );
        p.process_order.push(dropped);

        // The validator reports corruption rather than faulting on it: a binding
        // target past the last node used to index `seen_in_order` out of range.
        let past_the_end = NodeIdx(f.program.e_nodes.len() as u32);
        let b_input = f.program[nx(b)].inputs.start as usize;
        f.program.inputs[b_input].binding = ExecutionBinding::Bind(OutputAddr {
            node_idx: past_the_end,
            port_idx: 0,
        });
        assert_eq!(
            p.validate(&f.program).unwrap_err().to_string(),
            format!("execution order contains an out-of-range node index: {past_the_end:?}")
        );

        // Likewise for a set that no longer spans the program.
        f.program.inputs[b_input].binding = bind(a, 0);
        p.seeded.reset(0);
        assert_eq!(
            p.validate(&f.program).unwrap_err().to_string(),
            "schedule seeded spans 0 entries, not the program's 3"
        );
    }

    /// The other direction of the same invariant: reading a state the walk never
    /// settled is a broken schedule, so it fails loudly instead of answering as if
    /// the node were merely blocked. Checked on `input_missing`, the one such read
    /// that takes the column directly.
    #[test]
    fn reading_an_unvisited_producer_panics() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let mut f = Fix::default();
        let producer = f.node(false, &[], 1);
        let consumer = f.node(false, &[(false, bind(producer, 0))], 1);

        let mut schedule = RunSchedule::default();
        schedule.reset_for_program(&f.program);
        let consumer_input = &f.program.inputs[f.program[nx(consumer)].inputs][0];

        assert!(
            catch_unwind(AssertUnwindSafe(|| schedule.input_missing(consumer_input))).is_err(),
            "an unvisited producer is a broken schedule, not an unsatisfied input"
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
                p.states[nx(idx)].missing_required_inputs(),
                "node {idx:?} missing"
            );
            assert!(
                !p.states[nx(idx)].is_runnable(),
                "node {idx:?} not runnable"
            );
        }
    }

    #[test]
    fn optional_unbound_input_does_not_block() {
        // An *optional* unbound input is fine — the node still runs.
        let mut f = Fix::default();
        let a = f.node(true, &[(false, ExecutionBinding::None)], 1);

        let p = plan(&f);
        assert!(!p.states[nx(a)].missing_required_inputs());
        assert!(p.states[nx(a)].is_runnable());
        assert_eq!(p.process_order, [a].map(nx));
    }

    #[test]
    fn explicit_seed_overrides_disabled_dependency_for_this_run() {
        let mut f = Fix::default();
        let producer = f.node(false, &[], 1);
        f.program.by_id_mut(producer).disabled = true;
        let required = f.node(true, &[(true, bind(producer, 0))], 1);
        let optional = f.node(true, &[(false, bind(producer, 0))], 1);

        let mut planner = Planner::default();
        let mut plan = RunSchedule::default();
        planner
            .plan(
                &f.program,
                &RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                &mut plan,
            )
            .unwrap();
        assert_eq!(plan.states[nx(producer)], NodeState::Disabled);
        assert_eq!(plan.states[nx(required)], NodeState::MissingInputs);
        assert_eq!(plan.states[nx(optional)], NodeState::Cut);

        planner
            .plan(
                &f.program,
                &RunSeeds {
                    sinks: true,
                    e_node_ids: vec![producer],
                    ..Default::default()
                },
                &mut plan,
            )
            .unwrap();
        for e_node_id in [producer, required, optional] {
            assert_eq!(
                plan.states[nx(e_node_id)],
                NodeState::Cut,
                "the explicit producer seed makes every consumer runnable"
            );
        }
    }

    #[test]
    fn node_seed_is_both_a_root_and_seeded() {
        let mut f = Fix::default();
        let a = f.node(false, &[], 1);

        let mut planner = Planner::default();
        let mut p = RunSchedule::default();
        let seeds = RunSeeds {
            e_node_ids: vec![a],
            ..Default::default()
        };
        planner.plan(&f.program, &seeds, &mut p).expect("no cycle");

        assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(a)]);
        assert_eq!(p.roots.iter().collect::<Vec<_>>(), vec![nx(a)]);

        let seeds = RunSeeds {
            e_node_ids: vec![a, a],
            ..Default::default()
        };
        planner.plan(&f.program, &seeds, &mut p).expect("no cycle");
        assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(a)]);
        assert_eq!(p.roots.iter().collect::<Vec<_>>(), vec![nx(a)]);
    }

    #[test]
    fn dependency_cycle_is_rejected() {
        // A binds B, B binds A (A sink) — the planner must error, not loop.
        let mut f = Fix::default();
        f.node(true, &[(false, bind(ExecutionNodeId::from_u128(2), 0))], 1);
        f.node(false, &[(false, bind(ExecutionNodeId::from_u128(1), 0))], 1);

        let mut planner = Planner::default();
        let mut plan = RunSchedule::default();
        let seeds = RunSeeds {
            sinks: true,
            ..Default::default()
        };
        let result = planner.plan(&f.program, &seeds, &mut plan);
        assert!(matches!(result, Err(Error::CycleDetected { .. })));
    }

    #[test]
    fn node_seed_schedules_only_its_cone_and_pins_it() {
        // A → B → C (C sink). Seeding node B (by authoring id — top-level ids resolve
        // straight against the program) schedules only [A, B] — C is upstream of nothing
        // seeded — and records B as both a root and a seeded node. B's output has no
        // scheduled consumer.
        let mut f = Fix::default();
        let a = f.node(false, &[], 1);
        let b = f.node(false, &[(false, bind(a, 0))], 1);
        let c = f.node(true, &[(false, bind(b, 0))], 1);

        let mut planner = Planner::default();
        let mut p = RunSchedule::default();
        let seeds = RunSeeds {
            e_node_ids: vec![b],
            ..Default::default()
        };
        planner.plan(&f.program, &seeds, &mut p).expect("no cycle");

        assert_eq!(p.process_order, [a, b].map(nx), "only B's cone, deps first");
        assert_eq!(p.roots.iter().collect::<Vec<_>>(), vec![nx(b)]);
        assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(b)]);
        assert!(p.states[nx(a)].is_runnable());
        assert!(p.states[nx(b)].is_runnable());
        assert_eq!(
            p.states[nx(c)],
            NodeState::Unvisited,
            "C is upstream of nothing seeded, so the walk never reached it"
        );

        // Node seeds combine with sinks: the same seed plus `sinks` schedules
        // everything, and B stays seeded.
        let seeds = RunSeeds {
            sinks: true,
            e_node_ids: vec![b],
            ..Default::default()
        };
        planner.plan(&f.program, &seeds, &mut p).expect("no cycle");
        assert_eq!(p.process_order, [a, b, c].map(nx));
        assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(b)]);

        // A seed id absent from the program is inconsistent caller state — a hard failure,
        // not a silent skip.
        let bogus = ExecutionNodeId::from_u128(0xdead_beef);
        let seeds = RunSeeds {
            e_node_ids: vec![bogus],
            ..Default::default()
        };
        let err = planner.plan(&f.program, &seeds, &mut p).unwrap_err();
        assert!(matches!(err, Error::NodeSeedNotFound { e_node_id } if e_node_id == bogus));
    }

    #[test]
    fn event_seed_schedules_subscribers_and_rejects_missing_ports() {
        let mut f = Fix::default();
        let emitter = f.node(false, &[], 0);
        let subscriber = f.node(false, &[], 0);
        let events = f.program.events.append([ExecutionEvent {
            subscribers: vec![nx(subscriber)],
            ..Default::default()
        }]);
        f.program.by_id_mut(emitter).events = events;

        let event = ExecutionEventPort {
            e_node_id: emitter,
            event_idx: 0,
        };
        let mut planner = Planner::default();
        let mut plan = RunSchedule::default();
        planner
            .plan(
                &f.program,
                &RunSeeds {
                    events: vec![event],
                    ..Default::default()
                },
                &mut plan,
            )
            .unwrap();
        assert_eq!(plan.roots.iter().collect::<Vec<_>>(), vec![nx(subscriber)]);
        assert_eq!(plan.process_order, [subscriber].map(nx));
        assert!(plan.event_sources.iter().next().is_none());

        planner
            .plan(
                &f.program,
                &RunSeeds {
                    event_sources: true,
                    ..Default::default()
                },
                &mut plan,
            )
            .unwrap();
        assert_eq!(plan.roots.iter().collect::<Vec<_>>(), vec![nx(emitter)]);
        assert_eq!(
            plan.event_sources.iter().collect::<Vec<_>>(),
            vec![nx(emitter)]
        );
        assert_eq!(plan.process_order, [emitter].map(nx));

        let invalid = [
            ExecutionEventPort {
                e_node_id: ExecutionNodeId::from_u128(0xdead_beef),
                event_idx: 0,
            },
            ExecutionEventPort {
                e_node_id: emitter,
                event_idx: 1,
            },
        ];
        for event in invalid {
            let error = planner
                .plan(
                    &f.program,
                    &RunSeeds {
                        events: vec![event],
                        ..Default::default()
                    },
                    &mut plan,
                )
                .unwrap_err();
            assert!(
                matches!(error, Error::EventSeedNotFound { event: actual } if actual == event),
                "unexpected error for {event:?}: {error:?}"
            );
        }
    }
}

mod resolving {
    use std::sync::Arc;

    use crate::execution::cache::runtime::RuntimeCache;
    use crate::execution::cache::slot::{OutputSnapshot, ValueState};
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::index::{NodeIdx, OutputAddr};
    use crate::execution::program::{
        ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput, Program,
    };
    use crate::execution::schedule::{NodeState, RunSchedule, Scheduled};
    use crate::node::definition::{FuncBehavior, FuncId};
    use crate::node::lambda::{FuncLambda, OutputDemand};
    use crate::{DynamicValue, StaticValue, async_lambda};

    #[derive(Debug)]
    struct CachedNode {
        e_node_id: ExecutionNodeId,
        values: Vec<DynamicValue>,
    }

    #[derive(Default)]
    struct Fix {
        /// Shared like the compile artifact's, so it can be handed to
        /// [`RuntimeCache::reconcile`]; every read of it derefs to a `&Program`.
        program: Arc<Program>,
        order: Vec<ExecutionNodeId>,
    }

    impl Fix {
        /// The program while it is still exclusively this fixture's — every
        /// mutation below goes through here, and it stops being available the
        /// moment a cache is reconciled onto it.
        fn building(&mut self) -> &mut Program {
            Arc::get_mut(&mut self.program).expect("the fixture is built before it is shared")
        }

        fn node(&mut self, inputs: &[(bool, ExecutionBinding)], outputs: u32) -> ExecutionNodeId {
            let inputs = self
                .building()
                .inputs
                .append(inputs.iter().map(|(required, binding)| ExecutionInput {
                    required: *required,
                    stamps_fs_path: false,
                    binding: binding.clone(),
                }));
            let outputs = self
                .building()
                .outputs
                .append((0..outputs).map(|_| ExecutionOutput::default()));
            let idx = self.program.e_nodes.len();
            let e_node_id = ExecutionNodeId::from_u128(idx as u128 + 1);
            self.order.push(e_node_id);
            self.building().push(
                e_node_id,
                ExecutionNode {
                    behavior: FuncBehavior::Pure,
                    func_id: FuncId::from_u128(idx as u128 + 1),
                    inputs,
                    outputs,
                    lambda: async_lambda!(|_| { Ok(()) }),
                    ..Default::default()
                },
            );
            e_node_id
        }

        /// The schedule as it arrives at the sweep — what the planner would have left
        /// behind — swept to the one resolved run the executor reads.
        async fn resolve(
            &self,
            roots: &[ExecutionNodeId],
            seeded: &[ExecutionNodeId],
            missing: &[ExecutionNodeId],
            cached: Vec<CachedNode>,
        ) -> RunSchedule {
            let mut schedule = RunSchedule::default();
            schedule.reset_for_program(&self.program);
            schedule
                .process_order
                .extend(self.order.iter().map(|id| nx(*id)));
            // `Cut` is the planner's positive verdict: everything runnable but nothing claimed.
            schedule
                .states
                .reset(self.program.e_nodes.len(), NodeState::Cut);
            for e_node_id in missing {
                schedule.states[nx(*e_node_id)] = NodeState::MissingInputs;
            }
            for root in roots {
                schedule.roots.insert(nx(*root));
            }
            for seed in seeded {
                schedule.seeded.insert(nx(*seed));
            }
            let mut cache = RuntimeCache::default();
            cache.reconcile(&self.program);
            cache.stamp_digests(&self.program, schedule.executing());
            for cached in cached {
                let digest = cache[nx(cached.e_node_id)].current_digest.unwrap();
                cache[nx(cached.e_node_id)].value = ValueState::Resident {
                    snapshot: OutputSnapshot::new(cached.values),
                    produced_under: Some(digest),
                };
            }
            Scheduled::assume(&self.program, &mut schedule)
                .resolve(&mut cache)
                .await;
            schedule
        }
    }

    /// The fixture's id ↔ index invariant: ids are assigned `from_u128(idx + 1)`
    /// in push order, so a node's dense index is recoverable from its id.
    fn nx(e_node_id: ExecutionNodeId) -> NodeIdx {
        NodeIdx(e_node_id.as_uuid().as_u128() as u32 - 1)
    }

    fn bind(e_node_id: ExecutionNodeId, port_idx: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(OutputAddr {
            node_idx: nx(e_node_id),
            port_idx: port_idx as u32,
        })
    }

    fn value(value: i64) -> DynamicValue {
        DynamicValue::Static(StaticValue::Int(value))
    }

    #[tokio::test]
    async fn reuse_hit_prunes_its_whole_upstream_cone() {
        let mut fix = Fix::default();
        let source = fix.node(&[], 1);
        let cached = fix.node(&[(false, bind(source, 0))], 1);
        let sink = fix.node(&[(false, bind(cached, 0))], 0);

        let run = fix
            .resolve(
                &[sink],
                &[],
                &[],
                vec![CachedNode {
                    e_node_id: cached,
                    values: vec![value(1)],
                }],
            )
            .await;

        assert_eq!(run.states[nx(source)], NodeState::Cut);
        assert_eq!(run.states[nx(cached)], NodeState::Reuse);
        assert_eq!(run.states[nx(sink)], NodeState::Run);
        assert_eq!(
            run.outputs.readers.slice(fix.program.by_id(source).outputs),
            &[0]
        );
    }

    #[tokio::test]
    async fn exact_demand_accepts_narrow_producer_cache_and_ignores_reused_reader() {
        let mut fix = Fix::default();
        let source = fix.node(&[], 2);
        let cached = fix.node(&[(false, bind(source, 1))], 1);
        let live = fix.node(&[(false, bind(source, 0))], 1);
        let sink = fix.node(&[(false, bind(cached, 0)), (false, bind(live, 0))], 0);

        let run = fix
            .resolve(
                &[sink],
                &[],
                &[],
                vec![
                    CachedNode {
                        e_node_id: source,
                        values: vec![value(7), DynamicValue::Unbound],
                    },
                    CachedNode {
                        e_node_id: cached,
                        values: vec![value(8)],
                    },
                ],
            )
            .await;

        assert_eq!(run.states[nx(source)], NodeState::Reuse);
        assert_eq!(run.states[nx(cached)], NodeState::Reuse);
        assert_eq!(run.states[nx(live)], NodeState::Run);
        assert_eq!(run.states[nx(sink)], NodeState::Run);
        assert_eq!(
            run.outputs.demand.slice(fix.program.by_id(source).outputs),
            &[OutputDemand::Produce, OutputDemand::Skip]
        );
        assert_eq!(
            run.outputs.readers.slice(fix.program.by_id(source).outputs),
            &[1, 0]
        );
    }

    #[tokio::test]
    async fn missing_input_stops_liveness_before_its_producer() {
        let mut fix = Fix::default();
        let source = fix.node(&[], 1);
        let blocked = fix.node(
            &[(false, bind(source, 0)), (true, ExecutionBinding::None)],
            0,
        );

        let run = fix.resolve(&[blocked], &[], &[blocked], Vec::new()).await;

        assert_eq!(run.states[nx(source)], NodeState::Cut);
        assert_eq!(
            run.states[nx(blocked)],
            NodeState::MissingInputs,
            "a blocked root keeps the planner's verdict — the sweep refines only \
             runnable nodes, so the reason it did not run survives to the outcome"
        );
        assert_eq!(
            run.outputs.demand.slice(fix.program.by_id(source).outputs),
            &[OutputDemand::Skip]
        );
        assert_eq!(
            run.outputs.readers.slice(fix.program.by_id(source).outputs),
            &[0]
        );
    }

    #[tokio::test]
    async fn missing_lambda_stops_liveness_before_its_producer() {
        let mut fix = Fix::default();
        let source = fix.node(&[], 1);
        let missing = fix.node(&[(false, bind(source, 0))], 1);
        fix.building().by_id_mut(missing).lambda = FuncLambda::None;
        let sink = fix.node(&[(false, bind(missing, 0))], 0);

        let run = fix
            .resolve(
                &[sink],
                &[],
                &[],
                vec![CachedNode {
                    e_node_id: missing,
                    values: vec![value(9)],
                }],
            )
            .await;

        assert_eq!(run.states[nx(source)], NodeState::Cut);
        assert_eq!(
            run.states[nx(missing)],
            NodeState::MissingLambda,
            "a matching cache cannot hide a reached missing implementation"
        );
        assert_eq!(run.states[nx(sink)], NodeState::Run);
        assert_eq!(
            run.outputs.demand.slice(fix.program.by_id(source).outputs),
            &[OutputDemand::Skip]
        );
        assert_eq!(
            run.outputs.readers.slice(fix.program.by_id(source).outputs),
            &[0]
        );
        assert_eq!(
            run.outputs
                .readers
                .slice(fix.program.by_id(missing).outputs),
            &[1],
            "the downstream skip still owns one read to retire"
        );
    }

    /// A node seed demands every output it has, without any consumer reading them —
    /// the "run to this node" semantic, distinct from demand arriving through a
    /// binding.
    #[tokio::test]
    async fn a_node_seed_demands_every_output_without_readers() {
        let mut fix = Fix::default();
        let unseeded = fix.node(&[], 2);
        let seeded = fix.node(&[], 2);

        let run = fix
            .resolve(&[unseeded, seeded], &[seeded], &[], Vec::new())
            .await;

        assert_eq!(
            run.outputs
                .demand
                .slice(fix.program.by_id(unseeded).outputs),
            &[OutputDemand::Skip, OutputDemand::Skip],
            "a root nobody reads and nobody seeded produces nothing"
        );
        assert_eq!(
            run.outputs.demand.slice(fix.program.by_id(seeded).outputs),
            &[OutputDemand::Produce, OutputDemand::Produce]
        );
        assert!(run.outputs.readers.iter().all(|readers| *readers == 0));
    }

    #[tokio::test]
    async fn cone_reachable_only_through_a_reuse_hit_is_fully_pruned() {
        let mut fix = Fix::default();
        let deep = fix.node(&[], 1);
        let source = fix.node(&[(false, bind(deep, 0))], 1);
        let cached = fix.node(&[(false, bind(source, 0))], 1);
        let sink = fix.node(&[(false, bind(cached, 0))], 0);

        let run = fix
            .resolve(
                &[sink],
                &[],
                &[],
                vec![CachedNode {
                    e_node_id: cached,
                    values: vec![value(1)],
                }],
            )
            .await;

        assert_eq!(run.states[nx(deep)], NodeState::Cut);
        assert_eq!(run.states[nx(source)], NodeState::Cut);
        assert_eq!(run.states[nx(cached)], NodeState::Reuse);
        assert_eq!(run.states[nx(sink)], NodeState::Run);
    }
}
