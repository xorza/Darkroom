//! Both passes over the [`RunSchedule`](super::RunSchedule), each with the fixture
//! its own pass needs: a bare program for the structural walk, and a program plus a
//! primed cache for the sweep that reads one.

mod planning {
    use crate::execution::compile::compiled_graph::{ExecutionBinding, ExecutionEvent};
    use crate::execution::error::Error;
    use crate::execution::identity::{NodeIdx, OutputAddr, OutputIdx};
    use crate::execution::schedule::planner::Planner;
    use crate::execution::schedule::{NodeState, ResolvedOutputs, RootFlags, RunSchedule};
    use crate::execution::seeds::RunSeeds;
    use crate::graph::func::lambda::OutputDemand;
    use crate::graph::identity::{EventPort, NodeId};
    use crate::testing::program::ProgramBuilder;

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
        let mut prog = ProgramBuilder::default();
        let a = prog.node().outputs(2).add();
        prog.node().sink().input(a.out(0)).outputs(1).add();

        let mut planner = Planner::default();
        let mut schedule = RunSchedule::default();
        // A stale resolution from an earlier, differently shaped run.
        schedule.outputs.reset(1);
        schedule.outputs.add_reader(OutputIdx(0));

        planner
            .plan(prog.program(), &RunSeeds::sinks(), &mut schedule)
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
        let mut prog = ProgramBuilder::default();
        let a = prog.node().outputs(1).add();
        let b = prog.node().input(a.out(0)).outputs(1).add();
        let c = prog.node().sink().input(b.out(0)).outputs(1).add();

        let mut p = prog.plan_sinks();
        p.validate(prog.program()).unwrap();
        assert_eq!(
            p.process_order,
            [a, b, c].map(|node| node.node_idx),
            "post-order: deps first"
        );
        for idx in [a, b, c] {
            assert!(p.states[idx.node_idx].is_runnable());
            assert!(!p.states[idx.node_idx].missing_required_inputs());
        }

        p.process_order.swap(0, 1);
        assert_eq!(
            p.validate(prog.program()).unwrap_err().to_string(),
            format!(
                "execution node {:?} appears before dependency {:?}",
                b.node_id, a.node_id
            )
        );
        p.states[a.node_idx] = NodeState::Disabled;
        assert_eq!(
            p.validate(prog.program()).unwrap_err().to_string(),
            format!(
                "execution node {:?} appears before dependency {:?}",
                b.node_id, a.node_id
            ),
            "a disabled verdict cannot hide an enabled dependency"
        );
        p.process_order.swap(0, 1);
        // Back to what the planner actually wrote — not `default()`, which is the
        // `Unvisited` fill and would leave a scheduled node undecided.
        p.states[a.node_idx] = NodeState::Cut;
        p.validate(prog.program())
            .expect("restored to a valid plan");

        // Scheduling a node and deciding its state are one act, so a state may not
        // be decided for a node the schedule left out.
        let dropped = p
            .process_order
            .pop()
            .expect("the chain scheduled three nodes");
        assert_eq!(dropped, c.node_idx);
        assert_eq!(
            p.validate(prog.program()).unwrap_err().to_string(),
            format!("unscheduled node {:?} was decided Cut", c.node_id)
        );
        p.process_order.push(dropped);

        // The validator reports corruption rather than faulting on it: a binding
        // target past the last node used to index `seen_in_order` out of range.
        let past_the_end = NodeIdx(prog.program_mut().e_nodes.len() as u32);
        let b_input = prog.program()[b.node_idx].inputs.nth(0);
        prog.program_mut().inputs[b_input].binding = ExecutionBinding::Bind(OutputAddr {
            node_idx: past_the_end,
            port_idx: 0,
        });
        assert_eq!(
            p.validate(prog.program()).unwrap_err().to_string(),
            format!("execution order contains an out-of-range node index: {past_the_end:?}")
        );

        // Likewise for a set that no longer spans the program.
        prog.program_mut().inputs[b_input].binding = a.out(0);
        p.root_flags_mut().reset(0, RootFlags::default());
        assert_eq!(
            p.validate(prog.program()).unwrap_err().to_string(),
            "schedule root flags spans 0 entries, not the program's 3"
        );
    }

    /// The other direction of the same invariant: reading a state the walk never
    /// settled is a broken schedule, so it fails loudly instead of answering as if
    /// the node were merely blocked. Checked on `input_missing`, the one such read
    /// that takes the column directly.
    #[test]
    fn reading_an_unvisited_producer_panics() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let mut prog = ProgramBuilder::default();
        let producer = prog.node().outputs(1).add();
        let consumer = prog.node().input(producer.out(0)).outputs(1).add();

        let mut schedule = RunSchedule::default();
        schedule.reset_for_program(prog.program());
        let consumer_input = &prog.program().inputs[prog.program()[consumer.node_idx].inputs][0];

        assert!(
            catch_unwind(AssertUnwindSafe(|| schedule.input_missing(consumer_input))).is_err(),
            "an unvisited producer is a broken schedule, not an unsatisfied input"
        );
    }

    #[test]
    fn missing_required_input_blocks_node_and_dependents() {
        // A has a required *unbound* input ⇒ missing; B binds A ⇒ inherits missing.
        let mut prog = ProgramBuilder::default();
        let a = prog
            .node()
            .required(ExecutionBinding::None)
            .outputs(1)
            .add();
        let b = prog.node().sink().input(a.out(0)).outputs(1).add();

        let p = prog.plan_sinks();
        for idx in [a, b] {
            assert!(
                p.states[idx.node_idx].missing_required_inputs(),
                "node {idx:?} missing"
            );
            assert!(
                !p.states[idx.node_idx].is_runnable(),
                "node {idx:?} not runnable"
            );
        }
    }

    #[test]
    fn optional_unbound_input_does_not_block() {
        // An *optional* unbound input is fine — the node still runs.
        let mut prog = ProgramBuilder::default();
        let a = prog
            .node()
            .sink()
            .input(ExecutionBinding::None)
            .outputs(1)
            .add();

        let p = prog.plan_sinks();
        assert!(!p.states[a.node_idx].missing_required_inputs());
        assert!(p.states[a.node_idx].is_runnable());
        assert_eq!(p.process_order, [a].map(|node| node.node_idx));
    }

    #[test]
    fn explicit_seed_overrides_disabled_dependency_for_this_run() {
        let mut prog = ProgramBuilder::default();
        let producer = prog.node().outputs(1).add();
        prog.program_mut().by_id_mut(producer.node_id).disabled = true;
        let required = prog
            .node()
            .sink()
            .required(producer.out(0))
            .outputs(1)
            .add();
        let optional = prog.node().sink().input(producer.out(0)).outputs(1).add();

        let mut planner = Planner::default();
        let mut plan = RunSchedule::default();
        planner
            .plan(prog.program(), &RunSeeds::sinks(), &mut plan)
            .unwrap();
        assert_eq!(plan.states[producer.node_idx], NodeState::Disabled);
        assert_eq!(plan.states[required.node_idx], NodeState::MissingInputs);
        assert_eq!(plan.states[optional.node_idx], NodeState::Cut);

        planner
            .plan(
                prog.program(),
                &RunSeeds {
                    node_ids: vec![producer.node_id],
                    ..RunSeeds::sinks()
                },
                &mut plan,
            )
            .unwrap();
        for node_id in [producer, required, optional] {
            assert_eq!(
                plan.states[node_id.node_idx],
                NodeState::Cut,
                "the explicit producer seed makes every consumer runnable"
            );
        }
    }

    #[test]
    fn node_seed_is_both_a_root_and_seeded() {
        let mut prog = ProgramBuilder::default();
        let a = prog.node().outputs(1).add();

        let mut planner = Planner::default();
        let mut p = RunSchedule::default();
        let seeds = RunSeeds::nodes(vec![a.node_id]);
        planner
            .plan(prog.program(), &seeds, &mut p)
            .expect("no cycle");

        assert_eq!(p.seeded_roots(), vec![a.node_idx]);
        assert_eq!(p.roots(), [a.node_idx]);

        let seeds = RunSeeds::nodes(vec![a.node_id, a.node_id]);
        planner
            .plan(prog.program(), &seeds, &mut p)
            .expect("no cycle");
        assert_eq!(p.seeded_roots(), vec![a.node_idx]);
        assert_eq!(p.roots(), [a.node_idx], "a repeated seed is one root");
    }

    #[test]
    fn dependency_cycle_is_rejected() {
        // A binds B, B binds A (A sink) — the planner must error, not loop.
        // The forward reference is spelled by index, since the node it names
        // does not exist until the line after.
        let at = |node_idx: u32| {
            ExecutionBinding::Bind(OutputAddr {
                node_idx: NodeIdx(node_idx),
                port_idx: 0,
            })
        };
        let mut prog = ProgramBuilder::default();
        prog.node().sink().input(at(1)).outputs(1).add();
        prog.node().input(at(0)).outputs(1).add();

        let mut planner = Planner::default();
        let mut plan = RunSchedule::default();
        let seeds = RunSeeds::sinks();
        let result = planner.plan(prog.program(), &seeds, &mut plan);
        assert!(matches!(result, Err(Error::CycleDetected { .. })));
    }

    #[test]
    fn node_seed_schedules_only_its_cone_and_pins_it() {
        // A → B → C (C sink). Seeding node B (by authoring id, which resolves
        // straight against the program) schedules only [A, B] — C is upstream of nothing
        // seeded — and records B as both a root and a seeded node. B's output has no
        // scheduled consumer.
        let mut prog = ProgramBuilder::default();
        let a = prog.node().outputs(1).add();
        let b = prog.node().input(a.out(0)).outputs(1).add();
        let c = prog.node().sink().input(b.out(0)).outputs(1).add();

        let mut planner = Planner::default();
        let mut p = RunSchedule::default();
        let seeds = RunSeeds::nodes(vec![b.node_id]);
        planner
            .plan(prog.program(), &seeds, &mut p)
            .expect("no cycle");

        assert_eq!(
            p.process_order,
            [a, b].map(|node| node.node_idx),
            "only B's cone, deps first"
        );
        assert_eq!(p.roots(), [b.node_idx]);
        assert_eq!(p.seeded_roots(), vec![b.node_idx]);
        assert!(p.states[a.node_idx].is_runnable());
        assert!(p.states[b.node_idx].is_runnable());
        assert_eq!(
            p.states[c.node_idx],
            NodeState::Unvisited,
            "C is upstream of nothing seeded, so the walk never reached it"
        );

        // Node seeds combine with sinks: the same seed plus `sinks` schedules
        // everything, and B stays seeded.
        let seeds = RunSeeds {
            node_ids: vec![b.node_id],
            ..RunSeeds::sinks()
        };
        planner
            .plan(prog.program(), &seeds, &mut p)
            .expect("no cycle");
        assert_eq!(p.process_order, [a, b, c].map(|node| node.node_idx));
        assert_eq!(p.seeded_roots(), vec![b.node_idx]);

        // Seeding the sink itself is the case two root passes reach one node: the
        // seed pass marks C seeded, then the sinks sweep marks it a root again.
        // The second visit must *add* to what the first left — it is one root
        // carrying both facts, so C is listed once and stays seeded.
        let seeds = RunSeeds {
            node_ids: vec![c.node_id],
            ..RunSeeds::sinks()
        };
        planner
            .plan(prog.program(), &seeds, &mut p)
            .expect("no cycle");
        assert_eq!(p.roots(), [c.node_idx], "reached twice, listed once");
        assert_eq!(
            p.seeded_roots(),
            vec![c.node_idx],
            "the sinks sweep must not drop the seed the node already carried"
        );

        // A seed id absent from the program is inconsistent caller state — a hard failure,
        // not a silent skip.
        let bogus = NodeId::from_u128(0xdead_beef);
        let seeds = RunSeeds::nodes(vec![bogus]);
        let err = planner.plan(prog.program(), &seeds, &mut p).unwrap_err();
        assert!(matches!(err, Error::NodeSeedNotFound { node_id } if node_id == bogus));
    }

    #[test]
    fn event_seed_schedules_subscribers_and_rejects_missing_ports() {
        let mut prog = ProgramBuilder::default();
        let emitter = prog.node().outputs(0).add();
        let subscriber = prog.node().outputs(0).add();
        let events = prog.program_mut().events.append([ExecutionEvent {
            subscribers: vec![subscriber.node_idx],
            ..Default::default()
        }]);
        prog.program_mut().by_id_mut(emitter.node_id).events = events;

        let event = EventPort {
            node_id: emitter.node_id,
            event_idx: 0,
        };
        let mut planner = Planner::default();
        let mut plan = RunSchedule::default();
        planner
            .plan(prog.program(), &RunSeeds::events(vec![event]), &mut plan)
            .unwrap();
        assert_eq!(plan.roots(), [subscriber.node_idx]);
        assert_eq!(plan.process_order, [subscriber].map(|node| node.node_idx));
        assert!(plan.event_source_roots().is_empty());

        planner
            .plan(
                prog.program(),
                &RunSeeds {
                    event_sources: true,
                    ..Default::default()
                },
                &mut plan,
            )
            .unwrap();
        assert_eq!(plan.roots(), [emitter.node_idx]);
        assert_eq!(plan.event_source_roots(), vec![emitter.node_idx]);
        assert_eq!(plan.process_order, [emitter].map(|node| node.node_idx));

        // Seeded *and* an event source: the two properties are independent, so a
        // node reached as both keeps both — one root carrying two facts.
        planner
            .plan(
                prog.program(),
                &RunSeeds {
                    event_sources: true,
                    node_ids: vec![emitter.node_id],
                    ..Default::default()
                },
                &mut plan,
            )
            .unwrap();
        assert_eq!(plan.roots(), [emitter.node_idx]);
        assert_eq!(plan.seeded_roots(), vec![emitter.node_idx]);
        assert_eq!(plan.event_source_roots(), vec![emitter.node_idx]);

        let invalid = [
            EventPort {
                node_id: NodeId::from_u128(0xdead_beef),
                event_idx: 0,
            },
            EventPort {
                node_id: emitter.node_id,
                event_idx: 1,
            },
        ];
        for event in invalid {
            let error = planner
                .plan(prog.program(), &RunSeeds::events(vec![event]), &mut plan)
                .unwrap_err();
            assert!(
                matches!(error, Error::EventSeedNotFound { event: actual } if actual == event),
                "unexpected error for {event:?}: {error:?}"
            );
        }
    }
}

mod resolving {
    use crate::execution::compile::compiled_graph::ExecutionBinding;
    use crate::execution::schedule::NodeState;
    use crate::graph::func::lambda::{FuncLambda, OutputDemand};
    use crate::testing::program::ProgramBuilder;
    use crate::{DynamicValue, StaticValue};

    fn value(value: i64) -> DynamicValue {
        DynamicValue::Static(StaticValue::Int(value))
    }

    #[tokio::test]
    async fn reuse_hit_prunes_its_whole_upstream_cone() {
        let mut prog = ProgramBuilder::default();
        let source = prog.node().reusable().outputs(1).add();
        let cached = prog.node().reusable().input(source.out(0)).outputs(1).add();
        let sink = prog.node().reusable().input(cached.out(0)).outputs(0).add();

        let run = prog
            .sweep()
            .root(sink)
            .cached(cached, [value(1)])
            .run()
            .await;

        assert_eq!(run.state(source), NodeState::Cut);
        assert_eq!(run.state(cached), NodeState::Reuse);
        assert_eq!(run.state(sink), NodeState::Run);
        assert_eq!(run.readers(source), &[0]);
    }

    #[tokio::test]
    async fn exact_demand_accepts_narrow_producer_cache_and_ignores_reused_reader() {
        let mut prog = ProgramBuilder::default();
        let source = prog.node().reusable().outputs(2).add();
        let cached = prog.node().reusable().input(source.out(1)).outputs(1).add();
        let live = prog.node().reusable().input(source.out(0)).outputs(1).add();
        let sink = prog
            .node()
            .reusable()
            .input(cached.out(0))
            .input(live.out(0))
            .outputs(0)
            .add();

        let run = prog
            .sweep()
            .root(sink)
            .cached(source, [value(7), DynamicValue::Unbound])
            .cached(cached, [value(8)])
            .run()
            .await;

        assert_eq!(run.state(source), NodeState::Reuse);
        assert_eq!(run.state(cached), NodeState::Reuse);
        assert_eq!(run.state(live), NodeState::Run);
        assert_eq!(run.state(sink), NodeState::Run);
        assert_eq!(
            run.demand(source),
            &[OutputDemand::Produce, OutputDemand::Skip]
        );
        assert_eq!(run.readers(source), &[1, 0]);
    }

    #[tokio::test]
    async fn missing_input_stops_liveness_before_its_producer() {
        let mut prog = ProgramBuilder::default();
        let source = prog.node().reusable().outputs(1).add();
        let blocked = prog
            .node()
            .reusable()
            .input(source.out(0))
            .required(ExecutionBinding::None)
            .outputs(0)
            .add();

        let run = prog.sweep().root(blocked).missing(blocked).run().await;

        assert_eq!(run.state(source), NodeState::Cut);
        assert_eq!(
            run.state(blocked),
            NodeState::MissingInputs,
            "a blocked root keeps the planner's verdict — the sweep refines only \
             runnable nodes, so the reason it did not run survives to the outcome"
        );
        assert_eq!(run.demand(source), &[OutputDemand::Skip]);
        assert_eq!(run.readers(source), &[0]);
    }

    #[tokio::test]
    async fn missing_lambda_stops_liveness_before_its_producer() {
        let mut prog = ProgramBuilder::default();
        let source = prog.node().reusable().outputs(1).add();
        let missing = prog.node().reusable().input(source.out(0)).outputs(1).add();
        prog.program_mut().by_id_mut(missing.node_id).lambda = FuncLambda::None;
        let sink = prog
            .node()
            .reusable()
            .input(missing.out(0))
            .outputs(0)
            .add();

        let run = prog
            .sweep()
            .root(sink)
            .cached(missing, [value(9)])
            .run()
            .await;

        assert_eq!(run.state(source), NodeState::Cut);
        assert_eq!(
            run.state(missing),
            NodeState::MissingLambda,
            "a matching cache cannot hide a reached missing implementation"
        );
        assert_eq!(run.state(sink), NodeState::Run);
        assert_eq!(run.demand(source), &[OutputDemand::Skip]);
        assert_eq!(run.readers(source), &[0]);
        assert_eq!(
            run.readers(missing),
            &[1],
            "the downstream skip still owns one read to retire"
        );
    }

    /// A node seed demands every output it has, without any consumer reading them —
    /// the "run to this node" semantic, distinct from demand arriving through a
    /// binding.
    #[tokio::test]
    async fn a_node_seed_demands_every_output_without_readers() {
        let mut prog = ProgramBuilder::default();
        let unseeded = prog.node().reusable().outputs(2).add();
        let seeded = prog.node().reusable().outputs(2).add();

        let run = prog
            .sweep()
            .root(unseeded)
            .root(seeded)
            .seeded(seeded)
            .run()
            .await;

        assert_eq!(
            run.demand(unseeded),
            &[OutputDemand::Skip, OutputDemand::Skip],
            "a root nobody reads and nobody seeded produces nothing"
        );
        assert_eq!(
            run.demand(seeded),
            &[OutputDemand::Produce, OutputDemand::Produce]
        );
        assert!(
            run.schedule
                .outputs
                .readers
                .iter()
                .all(|readers| *readers == 0)
        );
    }

    #[tokio::test]
    async fn cone_reachable_only_through_a_reuse_hit_is_fully_pruned() {
        let mut prog = ProgramBuilder::default();
        let deep = prog.node().reusable().outputs(1).add();
        let source = prog.node().reusable().input(deep.out(0)).outputs(1).add();
        let cached = prog.node().reusable().input(source.out(0)).outputs(1).add();
        let sink = prog.node().reusable().input(cached.out(0)).outputs(0).add();

        let run = prog
            .sweep()
            .root(sink)
            .cached(cached, [value(1)])
            .run()
            .await;

        assert_eq!(run.state(deep), NodeState::Cut);
        assert_eq!(run.state(source), NodeState::Cut);
        assert_eq!(run.state(cached), NodeState::Reuse);
        assert_eq!(run.state(sink), NodeState::Run);
    }
}
