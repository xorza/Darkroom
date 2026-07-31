use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ::common::TempDir;

use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::execution::error::Error;
use crate::execution::report::NodeExecutionStatus;
use crate::execution::seeds::RunSeeds;
use crate::graph::func::error::InvokeError;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::{FuncLambda, Invocation};
use crate::graph::identity::{EventPort, NodeId};
use crate::graph::node::CacheMode;
use crate::testing::TestFuncHooks;
use crate::testing::calls::Calls;
use crate::testing::graph::TestGraph;
use crate::testing::worker::TestWorker;
use crate::worker::Worker;
use crate::worker::error::WorkerError;
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::status::{WorkerActivity, WorkerStatusKind};
use crate::{DataType, StaticValue, async_lambda};

/// How long a "nothing happens" claim watches for before it is believed.
const QUIET: Duration = Duration::from_millis(100);

mod runs {
    use super::*;

    /// The frame counter advances once per firing, and every firing runs the
    /// whole subscribed cone.
    #[tokio::test]
    async fn each_frame_event_runs_the_subscribed_cone() {
        let mut w = TestWorker::frames();
        w.send_many([w.update(), w.fire("Frame Event", 0)]);

        for expected in ["1", "2", "3"] {
            if expected != "1" {
                w.send(w.fire("Frame Event", 0));
            }
            let run = w.run().await;
            assert_eq!(run.ran_node_count, 3);
            assert_eq!(run.logs(), [expected]);
        }
    }

    #[tokio::test]
    async fn events_are_deduplicated() {
        let mut w = TestWorker::frames();
        w.send(w.update());

        let event = w.event("Frame Event", 0);
        w.send(WorkerMessage::Run {
            seeds: RunSeeds::events(vec![event, event, event]),
        });

        assert_eq!(w.run().await.logs(), ["1"]);
    }

    #[tokio::test]
    async fn sink_seeds_run_the_sink() {
        let mut w = TestWorker::printing("hello");
        w.send_many([w.update(), TestWorker::sinks()]);

        let run = w.run().await;

        assert_eq!(run.ran_node_count, 1);
        assert_eq!(run.logs(), ["hello"]);
    }

    /// Node seeds end-to-end: the seed overrides a compiled disabled node and
    /// the worker runs only its cone — the sink `Print` panics if reached.
    #[tokio::test]
    async fn node_seeds_override_a_disabled_node_and_run_only_its_cone() {
        let mut graph = TestGraph::sample_with(TestFuncHooks {
            get_a: Arc::new(|| Ok(1)),
            get_b: Arc::new(|| 11),
            ..Default::default()
        });
        graph.cache_all(CacheMode::None);
        graph.disable("sum");
        let mut w = TestWorker::over(graph);
        let sum = w.id("sum");

        w.send_many([
            w.update(),
            WorkerMessage::Run {
                seeds: RunSeeds::nodes(vec![sum]),
            },
        ]);

        let run = w.run().await;
        assert_eq!(
            run.ran(),
            ["get_a", "get_b", "sum"],
            "only the disabled sum's cone ran"
        );
        assert!(
            w.graph.graph.find(sum).unwrap().disabled,
            "execution does not mutate the authoring graph"
        );
    }

    /// A compiled disabled sink must not participate in an ordinary sink run.
    /// The default hooks panic if any node executes.
    #[tokio::test]
    async fn a_disabled_sink_stays_out_of_sink_runs() {
        let mut graph = TestGraph::sample_with(TestFuncHooks::default());
        graph.disable("Print");
        let mut w = TestWorker::over(graph);

        w.send_many([w.update(), TestWorker::sinks()]);

        let run = w.run().await;
        assert_eq!(run.ran_node_count, 0);
        assert!(run.missing_inputs().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancel_with_no_active_run_does_not_reach_the_next_one() {
        let mut w = TestWorker::printing("hi");

        w.worker.request_cancel();
        w.send_many([w.update(), TestWorker::sinks()]);

        let run = w.run().await;
        assert!(!run.cancelled, "an idle cancel affected the next run");
        assert_eq!(run.ran_node_count, 1, "the run completed in full");
    }

    #[tokio::test]
    async fn sync_fires_after_execution() {
        let mut w = TestWorker::frames();

        w.settle([w.update(), w.fire("Frame Event", 0)]).await;

        w.run().await;
    }
}

mod live_progress {
    use super::*;

    /// A run's progress reaches the host as it happens: the install first, then
    /// the switch to `Executing`, then one `Running` and one `Executed` patch
    /// for the node, and only then the completion.
    #[tokio::test(flavor = "multi_thread")]
    async fn node_patches_stream_before_completion() {
        let mut w = TestWorker::printing("hi");
        let compiled = w.compile();
        let print = w.id("Print");
        w.send_many([
            WorkerMessage::Update {
                compiled: Arc::clone(&compiled),
            },
            TestWorker::sinks(),
        ]);

        let mut started = 0;
        let mut node_finished = 0;
        let mut installed = false;
        let mut execution_started = false;
        loop {
            match w.report().await {
                WorkerReport::Installed(program) => {
                    assert!(!installed, "one update installed more than once");
                    assert!(Arc::ptr_eq(&program, &compiled));
                    installed = true;
                }
                WorkerReport::Status(status)
                    if status.kind == WorkerStatusKind::Activity
                        && status.activity == WorkerActivity::Executing =>
                {
                    assert!(installed, "execution started before installation");
                    assert!(!execution_started, "execution started more than once");
                    execution_started = true;
                }
                WorkerReport::Status(status) if status.kind == WorkerStatusKind::Patch => {
                    assert!(execution_started, "node patch arrived before execution");
                    assert_eq!(status.activity, WorkerActivity::Executing);
                    for node in &status.nodes {
                        assert_eq!(node.node_id, print, "status maps to the node");
                        assert!(compiled.contains(node.node_id));
                        match node.status {
                            Some(NodeExecutionStatus::Running { .. }) => started += 1,
                            Some(NodeExecutionStatus::Executed { .. }) => node_finished += 1,
                            ref unexpected => panic!("unexpected live node status: {unexpected:?}"),
                        }
                    }
                }
                WorkerReport::Status(status)
                    if matches!(status.kind, WorkerStatusKind::Completed { .. }) =>
                {
                    assert!(installed, "completion arrived before installation");
                    assert_eq!(status.activity, WorkerActivity::Idle);
                    assert_eq!(started, 1, "one running update before completion");
                    assert_eq!(node_finished, 1, "one executed update before completion");
                    break;
                }
                WorkerReport::Status(status) => panic!("unexpected worker status: {status:?}"),
                WorkerReport::Cleared => panic!("unexpected clear"),
                WorkerReport::Error(error) => panic!("unexpected worker error: {error}"),
            }
        }

        w.send(WorkerMessage::Clear);
        assert!(matches!(w.report().await, WorkerReport::Cleared));
    }

    /// A → B with trivial sync lambdas, which give the run future no suspension
    /// point of their own: nothing but direct reporting can get their progress
    /// out mid-run. B records how many node-status patch entries the host
    /// callback has already seen — A's `Running` and `Executed` *and* B's own
    /// `Running` must all have reached the host by the time B's lambda runs.
    ///
    /// The callback itself is the subject here, so this one wires a raw
    /// [`Worker`] rather than going through the harness.
    #[tokio::test(flavor = "multi_thread")]
    async fn live_patches_reach_the_host_before_downstream_nodes_run() {
        let patch_entries = Arc::new(AtomicU64::new(0));
        let seen_by_second = Arc::new(AtomicU64::new(u64::MAX));

        let mut graph = TestGraph::new();
        graph.add("first", |node| {
            node.output(DataType::Int).compute(|_| StaticValue::Int(1))
        });
        graph.add("second", |node| {
            let seen = Arc::clone(&seen_by_second);
            let entries = Arc::clone(&patch_entries);
            node.sink()
                .input(DataType::Int)
                .lambda(async_lambda!(move |_| {
                    seen = Arc::clone(&seen),
                    entries = Arc::clone(&entries)
                } => {
                    seen.store(entries.load(Ordering::SeqCst), Ordering::SeqCst);
                    Ok(())
                }))
        });
        graph.wire("first", 0, "second", 0);
        let compiled = TestWorker::over(graph).compile();

        let entries = Arc::clone(&patch_entries);
        let (tx, mut rx) = mpsc::unbounded_channel::<WorkerReport>();
        let worker = Worker::new(move |report| {
            if let WorkerReport::Status(status) = &report
                && status.kind == WorkerStatusKind::Patch
            {
                entries.fetch_add(status.nodes.len() as u64, Ordering::SeqCst);
            }
            tx.send(report).ok();
        });
        worker
            .send_many([WorkerMessage::Update { compiled }, TestWorker::sinks()])
            .unwrap();

        loop {
            let report = timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("worker timed out")
                .expect("worker channel closed");
            if let WorkerReport::Status(status) = report
                && matches!(status.kind, WorkerStatusKind::Completed { .. })
            {
                break;
            }
        }
        assert_eq!(
            seen_by_second.load(Ordering::SeqCst),
            3,
            "the first node's Running and Executed patches, and the second's own Running, must \
             reach the host before the second node's lambda runs"
        );
    }

    #[tokio::test]
    async fn activity_is_reported_absolutely_and_in_order() {
        let mut w = TestWorker::frames();

        w.settle([w.update(), WorkerMessage::StartEventLoop]).await;

        let mut activities = Vec::new();
        while activities.last() != Some(&WorkerActivity::EventLoop) {
            let status = w.status().await;
            if activities.last() != Some(&status.activity) {
                activities.push(status.activity);
            }
        }
        assert_eq!(
            activities,
            [WorkerActivity::Executing, WorkerActivity::EventLoop]
        );

        w.settle([WorkerMessage::StopEventLoop]).await;
        loop {
            let status = w.status().await;
            if status.kind == WorkerStatusKind::Activity && status.activity == WorkerActivity::Idle
            {
                break;
            }
        }
    }
}

mod batching {
    use super::*;

    #[tokio::test]
    async fn clear_resets_the_execution_graph() {
        let mut w = TestWorker::frames();
        w.send_many([w.update(), w.fire("Frame Event", 0)]);
        assert_eq!(w.run().await.logs(), ["1"]);

        w.send_many([WorkerMessage::Clear, w.fire("Frame Event", 0)]);

        // After Clear the frame event has no subscribers, so nothing runs.
        w.nothing_runs_within(QUIET).await;
    }

    /// Scan-then-commit ordering: `Clear` zeroes the execution graph, `Update`
    /// queues a replacement, and the commit phase applies the update — so the
    /// event still executes.
    #[tokio::test]
    async fn clear_then_update_in_one_batch_applies_the_update() {
        let mut w = TestWorker::frames();

        w.send_many([WorkerMessage::Clear, w.update(), w.fire("Frame Event", 0)]);

        let run = w.run().await;
        assert_eq!(run.ran_node_count, 3);
        assert_eq!(run.logs(), ["1"]);
    }

    #[tokio::test]
    async fn update_then_clear_in_one_batch_leaves_the_graph_cleared() {
        let mut w = TestWorker::frames();

        w.send_many([w.update(), WorkerMessage::Clear, w.fire("Frame Event", 0)]);

        w.nothing_runs_within(QUIET).await;
    }

    /// An empty batch must not panic, hang, or desynchronize the worker.
    #[tokio::test]
    async fn an_empty_batch_is_a_noop() {
        let w = TestWorker::over(TestGraph::new());

        w.send_many(std::iter::empty::<WorkerMessage>());

        // A subsequent Sync still fires, so the worker is alive.
        w.sync().await;
    }

    #[tokio::test]
    async fn every_sync_in_a_batch_fires() {
        let w = TestWorker::over(TestGraph::new());
        let (reply_a, ack_a) = oneshot::channel();
        let (reply_b, ack_b) = oneshot::channel();

        w.send_many([
            WorkerMessage::Sync { reply: reply_a },
            WorkerMessage::Sync { reply: reply_b },
        ]);

        for (ack, which) in [(ack_a, "first"), (ack_b, "second")] {
            timeout(Duration::from_millis(500), ack)
                .await
                .unwrap_or_else(|_| panic!("the {which} Sync never fired"))
                .unwrap_or_else(|_| panic!("the {which} sender was dropped"));
        }
    }

    /// Messages that become ready at the same wake reduce into one batch, so
    /// the superseded update never runs.
    #[tokio::test(flavor = "current_thread")]
    async fn messages_ready_at_one_wake_reduce_together() {
        let mut w = TestWorker::printing("first");
        let first = w.compile();
        w.send(WorkerMessage::Update {
            compiled: Arc::clone(&first),
        });
        w.send(TestWorker::sinks());

        w.graph = TestWorker::print_graph("second");
        let second = w.compile();
        let second_print = w.id("Print");
        w.send(WorkerMessage::Update {
            compiled: Arc::clone(&second),
        });
        w.send(TestWorker::sinks());

        let run = w.run().await;
        assert!(!Arc::ptr_eq(w.installed(), &first));
        assert!(Arc::ptr_eq(w.installed(), &second));
        assert!(w.installed().contains(second_print));
        assert_eq!(run.logs(), ["second"]);
        w.nothing_runs_within(QUIET).await;
    }
}

/// Running on a worker holding no graph is an ordinary state, not a failure:
/// the worker skips execution silently and publishes nothing at all.
mod empty_graph {
    use super::*;

    #[tokio::test]
    async fn sink_seeds_are_a_silent_noop() {
        let mut w = TestWorker::over(TestGraph::new());

        w.send(TestWorker::sinks());

        w.nothing_runs_within(QUIET).await;
    }

    #[tokio::test]
    async fn event_seeds_are_a_silent_noop() {
        let mut w = TestWorker::over(TestGraph::new());

        w.send(WorkerMessage::Run {
            seeds: RunSeeds::events(vec![EventPort {
                node_id: NodeId::unique(),
                event_idx: 0,
            }]),
        });

        w.nothing_runs_within(QUIET).await;
    }

    #[tokio::test]
    async fn starting_the_event_loop_is_a_silent_noop() {
        let mut w = TestWorker::over(TestGraph::new());

        w.settle([WorkerMessage::StartEventLoop]).await;

        w.nothing_runs_within(QUIET).await;
    }

    #[tokio::test]
    async fn sink_seeds_with_start_event_loop_are_a_silent_noop() {
        let mut w = TestWorker::over(TestGraph::new());

        w.settle([TestWorker::sinks(), WorkerMessage::StartEventLoop])
            .await;

        w.nothing_runs_within(QUIET).await;
    }
}

mod event_loop {
    use super::*;

    #[tokio::test]
    async fn start_then_stop() {
        let mut w = TestWorker::frames();

        w.settle([w.update(), WorkerMessage::StartEventLoop]).await;

        // The bootstrap run initializes the event source and logs nothing; the
        // loop's first firing then runs the whole cone.
        assert!(w.run().await.logs().is_empty());
        let fired = w.run().await;
        assert_eq!(fired.ran_node_count, 3);
        assert_eq!(fired.logs().len(), 1);

        w.settle([WorkerMessage::StopEventLoop]).await;
        w.drain();
        w.nothing_runs_within(QUIET).await;
    }

    #[tokio::test]
    async fn stopping_a_loop_that_is_not_running_is_a_noop() {
        let w = TestWorker::over(TestGraph::new());

        w.settle([WorkerMessage::StopEventLoop]).await;
    }

    /// A batch triggering both an execution and `StartEventLoop` must run
    /// execute once and publish one completion, not two. A sink-only graph
    /// prepares no event triggers, so the loop never actually starts — which
    /// removes lambda-driven runs as a confounding factor while still
    /// exercising the rebuild transition.
    #[tokio::test]
    async fn sink_seeds_with_start_event_loop_complete_once() {
        let mut w = TestWorker::printing("hi");

        w.send_many([
            w.update(),
            TestWorker::sinks(),
            WorkerMessage::StartEventLoop,
        ]);

        assert_eq!(w.run().await.logs(), ["hi"]);
        w.nothing_runs_within(QUIET).await;
    }

    /// An `Update` while the loop is running restarts it over the new program.
    #[tokio::test]
    async fn update_restarts_a_running_loop() {
        let mut w = TestWorker::frames();

        w.settle([w.update(), WorkerMessage::StartEventLoop]).await;
        w.drain();
        w.run().await;

        w.settle([w.update()]).await;
        w.drain();
        assert_eq!(w.run().await.ran_node_count, 3);
    }

    /// `StartEventLoop` while a loop is already running stops the current one
    /// before rebuilding it, rather than panicking or leaking it.
    #[tokio::test]
    async fn starting_twice_is_idempotent() {
        let mut w = TestWorker::frames();

        w.settle([w.update(), WorkerMessage::StartEventLoop]).await;
        w.drain();
        w.run().await;

        w.settle([WorkerMessage::StartEventLoop]).await;
        w.drain();
        w.run().await;
    }

    /// End-to-end: an event fired by a lambda reaches the worker's execute path
    /// and produces a completion, over the dedicated bounded channel.
    #[tokio::test]
    async fn lambda_events_drive_execution() {
        let mut w = TestWorker::frames();

        w.send_many([w.update(), WorkerMessage::StartEventLoop]);

        // The first completion is the bootstrap run inside the start path; the
        // second can only come from the lambda firing.
        w.run().await;
        w.run().await;
    }

    /// The lambda fires as fast as it can; a `StopEventLoop` must still be
    /// observed within a bounded time rather than being starved by the event
    /// stream.
    #[tokio::test]
    async fn commands_are_not_starved_by_a_fast_loop() {
        let mut w = TestWorker::frames();

        w.settle([w.update(), WorkerMessage::StartEventLoop]).await;
        w.run().await;
        w.run().await;
        w.drain();

        w.settle([WorkerMessage::StopEventLoop]).await;

        w.drain();
        w.nothing_runs_within(QUIET).await;
    }

    /// A firing runs only the subscriber, never re-initializing the event
    /// sources: `source_b`'s own lambda must not be invoked again because
    /// `source_a` ticked.
    #[tokio::test]
    async fn a_fired_event_does_not_reinitialize_the_event_sources() {
        let (a_calls, b_calls, subscriber_calls) =
            (Calls::default(), Calls::default(), Calls::default());
        let a_notify = Arc::new(Notify::new());
        let b_notify = Arc::new(Notify::new());

        let mut graph = TestGraph::new();
        for (name, notify, counter) in [
            ("source_a", &a_notify, &a_calls),
            ("source_b", &b_notify, &b_calls),
        ] {
            let notify = Arc::clone(notify);
            let counter = counter.clone();
            graph.add(name, move |node| {
                node.event(
                    "tick",
                    EventLambda::new(move |_state| {
                        let notify = Arc::clone(&notify);
                        Box::pin(async move { notify.notified().await })
                    }),
                )
                .lambda(async_lambda!(move |_| { counter = counter.clone() } => {
                    counter.bump();
                    Ok(())
                }))
            });
        }
        let counter = subscriber_calls.clone();
        graph.add("subscriber", move |node| {
            node.lambda(async_lambda!(move |_| { counter = counter.clone() } => {
                counter.bump();
                Ok(())
            }))
        });
        graph.subscribe("source_a", 0, "subscriber");
        graph.subscribe("source_b", 0, "subscriber");

        let mut w = TestWorker::over(graph);
        w.send_many([w.update(), WorkerMessage::StartEventLoop]);

        let bootstrap = w.run().await;
        assert_eq!(bootstrap.ran(), ["source_a", "source_b"]);
        assert_eq!(subscriber_calls.count(), 0);

        a_notify.notify_one();
        let fired = w.run().await;
        assert_eq!(fired.ran(), ["subscriber"], "only the subscriber re-ran");
        assert_eq!(a_calls.count(), 1);
        assert_eq!(b_calls.count(), 1);
        assert_eq!(subscriber_calls.count(), 1);

        w.settle([WorkerMessage::StopEventLoop]).await;
    }

    /// One event task panicking stops the whole loop and is attributed to its
    /// node, even while a sibling task is still parked.
    #[tokio::test]
    async fn one_task_panicking_stops_the_loop() {
        let mut w = TestWorker::frames();
        w.graph.edit_func("Frame Event", |func| {
            func.events[0].event_lambda =
                EventLambda::new(|_state| Box::pin(async { panic!("event loop stopped") }));
            func.events[1].event_lambda =
                EventLambda::new(|_state| Box::pin(std::future::pending()));
        });
        w.graph.subscribe("Frame Event", 1, "Print");
        let frame_event = w.id("Frame Event");

        w.send_many([w.update(), WorkerMessage::StartEventLoop]);

        let mut activities = Vec::new();
        loop {
            match w.report().await {
                WorkerReport::Status(status) => {
                    if activities.last() != Some(&status.activity) {
                        activities.push(status.activity);
                    }
                }
                WorkerReport::Error(WorkerError::Execution {
                    error: Error::EventLambdaPanic { node_id, message },
                }) => {
                    assert_eq!(node_id, frame_event);
                    assert!(message.contains("event loop stopped"));
                    break;
                }
                WorkerReport::Installed(_)
                | WorkerReport::Cleared
                | WorkerReport::Error(WorkerError::Execution { .. })
                | WorkerReport::Error(WorkerError::CacheEviction { .. }) => {}
            }
        }
        assert_eq!(
            activities,
            [
                WorkerActivity::Executing,
                WorkerActivity::EventLoop,
                WorkerActivity::Idle,
            ]
        );
        w.sync().await;
    }
}

mod replacement {
    use super::*;

    /// A run seeded with an event the *replacing* program no longer holds is
    /// refused, and the worker carries on with the new program.
    #[tokio::test]
    async fn a_stale_event_seed_is_rejected_without_stopping_the_worker() {
        let mut w = TestWorker::frames();
        w.settle([w.update()]).await;
        let stale = w.event("Frame Event", 0);

        w.graph = TestWorker::print_graph("replacement");
        w.send_many([
            w.update(),
            WorkerMessage::Run {
                seeds: RunSeeds::events(vec![stale]),
            },
        ]);

        let error = w.finished().await.expect_err("the stale seed is refused");
        assert!(
            matches!(error, Error::EventSeedNotFound { event } if event == stale),
            "unexpected stale-event error: {error:?}"
        );

        w.send(TestWorker::sinks());
        assert_eq!(w.run().await.logs(), ["replacement"]);
    }

    /// An `Update` arriving mid-run is installed only after the running program
    /// reports — so the completion still describes the program that ran.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_replacement_queued_mid_run_is_installed_after_the_running_program() {
        use std::sync::{Condvar, Mutex};

        let started = Arc::new(Notify::new());
        let release = Arc::new((Mutex::new(false), Condvar::new()));

        let mut graph = TestGraph::new();
        graph.add("source", |node| {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            node.output(DataType::Int).compute(move |_| {
                started.notify_one();
                let (lock, wake) = &*release;
                let held = lock.lock().unwrap();
                drop(wake.wait_while(held, |released| !*released).unwrap());
                StaticValue::Int(7)
            })
        });
        graph.add("sink", |node| node.records());
        graph.wire("source", 0, "sink", 0);

        let mut w = TestWorker::over(graph);
        let source = w.id("source");
        let sink = w.id("sink");
        let running = w.compile();
        w.send_many([
            WorkerMessage::Update {
                compiled: Arc::clone(&running),
            },
            TestWorker::sinks(),
        ]);
        timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("the run did not start");

        w.graph = TestWorker::print_graph("next");
        let replacement = w.compile();
        let replacement_node = w.id("Print");
        w.send(WorkerMessage::Update {
            compiled: Arc::clone(&replacement),
        });
        {
            let (lock, wake) = &*release;
            *lock.lock().unwrap() = true;
            wake.notify_one();
        }
        w.sync().await;

        w.finished().await.expect("the running program completed");
        assert!(Arc::ptr_eq(w.installed(), &running));
        assert!(w.installed().contains(source));
        assert!(w.installed().contains(sink));
        assert!(!w.installed().contains(replacement_node));

        let WorkerReport::Installed(installed) = w.report().await else {
            panic!("the replacement installed before the running program finished");
        };
        assert!(Arc::ptr_eq(&installed, &replacement));
    }
}

mod cache {
    use super::*;

    /// Eviction is fire-and-forget: it happens inside the batch, before the
    /// acknowledgement, and reports nothing when it succeeds.
    #[tokio::test]
    async fn a_successful_eviction_reports_nothing() {
        let mut w = TestWorker::over(TestGraph::sample_with(TestFuncHooks::default()));
        let compiled = w.compile();
        let get_a = w.id("get_a");

        w.settle([
            WorkerMessage::Update {
                compiled: Arc::clone(&compiled),
            },
            WorkerMessage::EvictCache { nodes: vec![get_a] },
        ])
        .await;

        let WorkerReport::Installed(installed) = w.report().await else {
            panic!("installation must be reported before cache eviction");
        };
        assert!(Arc::ptr_eq(&installed, &compiled));
        w.quiet();
    }

    #[tokio::test]
    async fn an_eviction_failure_uses_the_general_worker_error_report() {
        let dir = TempDir::new("eviction-error");
        let mut w = TestWorker::over(TestGraph::sample_with(TestFuncHooks::default()));
        let blocked = w.id("get_a");
        // A directory where the blob file belongs: removal fails on it.
        let blocked_path = dir.join(blocked.as_uuid().simple().to_string());
        std::fs::create_dir(&blocked_path).unwrap();

        w.settle([
            w.disk_store(dir.path()),
            w.update(),
            WorkerMessage::EvictCache {
                nodes: vec![blocked],
            },
        ])
        .await;

        assert!(matches!(w.report().await, WorkerReport::Installed(_)));
        let WorkerReport::Error(WorkerError::CacheEviction {
            failure_count,
            details,
        }) = w.report().await
        else {
            panic!("a cache deletion failure must use the general worker error report");
        };
        assert_eq!(failure_count, 1);
        assert!(details.contains(&format!("{blocked:?}")));
        assert!(details.contains(&format!("failed to remove {}", blocked_path.display())));
        w.quiet();
    }

    /// `source` (pure, RAM) → `square` (pure, Disk) → `print` (sink).
    fn disk_cached_graph(calls: &Calls) -> TestGraph {
        let mut graph = TestGraph::new();
        let source = calls.returning(7i64);
        graph.add("source", move |node| {
            node.pure()
                .cache(CacheMode::Ram)
                .output(DataType::Int)
                .compute(source)
        });
        graph.add("square", |node| {
            node.pure()
                .cache(CacheMode::Disk)
                .input(DataType::Int)
                .output(DataType::Int)
                .compute(|inputs| {
                    let value = inputs[0].as_i64().unwrap();
                    StaticValue::Int(value * value)
                })
        });
        graph.add("print", |node| node.records());
        graph.wire("source", 0, "square", 0);
        graph.wire("square", 0, "print", 0);
        graph
    }

    /// The disk cache wires through `SetDiskStore` and persists across worker
    /// restarts: a `Disk` reproducible node's output, stored on a cold run,
    /// reloads on a fresh worker over the same store so its upstream never
    /// recomputes. `SetDiskStore` shares the batch with `Update`, proving it is
    /// applied before the install hydrates.
    #[tokio::test]
    async fn a_disk_cached_node_survives_a_worker_restart() {
        let dir = TempDir::new("diskcache");
        let calls = Calls::default();

        let mut w = TestWorker::over(disk_cached_graph(&calls));
        w.send_many([w.disk_store(dir.path()), w.update(), TestWorker::sinks()]);
        let cold = w.run().await;
        assert_eq!(
            cold.ran(),
            ["print", "source", "square"],
            "a cold run computes every node"
        );
        assert_eq!(calls.count(), 1);
        assert_eq!(cold.logs(), ["49"]);

        // Reopen on a fresh worker over the same store: `square` loads from
        // disk and is reused. Its input `source` feeds only the reused
        // `square`, which never reads it, so the pre-run cut prunes it.
        let mut w = w.restart();
        w.send_many([w.disk_store(dir.path()), w.update(), TestWorker::sinks()]);
        let warm = w.run().await;
        assert_eq!(
            calls.count(),
            1,
            "the cut prunes the Ram input feeding only a disk-cache hit"
        );
        assert_eq!(warm.ran(), ["print"], "only the sink re-ran");
        assert_eq!(
            warm.cached(),
            ["square"],
            "square is served from the disk cache"
        );
        assert_eq!(warm.logs(), ["49"]);
    }

    /// `SetDiskStore` flushes resident disk-backed values into the just-attached
    /// store: a `Both`-mode value computed while no store root existed (an
    /// unsaved document) would otherwise be a RAM hit on every later run —
    /// which never stores — and silently recompute on reopen.
    #[tokio::test]
    async fn attaching_a_store_flushes_resident_disk_backed_values() {
        let dir = TempDir::new("storeswap");
        let calls = Calls::default();
        let mut graph = disk_cached_graph(&calls);
        graph.cache("square", CacheMode::Both);

        // Run with no store root: the Both value stays resident-only.
        let mut w = TestWorker::over(graph);
        w.send_many([w.update(), TestWorker::sinks()]);
        w.run().await;
        assert_eq!(dir.entry_count(), 0);

        w.settle([w.disk_store(dir.path())]).await;

        assert_eq!(
            dir.entry_count(),
            1,
            "the resident Both-mode value was flushed into the new store"
        );
    }
}

mod shutdown {
    use super::*;

    /// `exit` drains the event tasks — dropping their futures — and publishes
    /// idle before returning, after which the worker refuses messages.
    #[tokio::test]
    async fn exit_waits_for_active_event_cleanup_and_the_idle_report() {
        #[derive(Debug)]
        struct EventFutureDrop(Arc<AtomicBool>);

        impl Drop for EventFutureDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let entered = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let mut w = TestWorker::frames();
        w.graph.edit_func("Frame Event", |func| {
            let entered = Arc::clone(&entered);
            let dropped = Arc::clone(&dropped);
            func.events[0].event_lambda = EventLambda::new(move |_state| {
                let entered = Arc::clone(&entered);
                let dropped = Arc::clone(&dropped);
                Box::pin(async move {
                    let _drop = EventFutureDrop(dropped);
                    entered.notify_one();
                    std::future::pending::<()>().await;
                })
            });
        });

        w.settle([w.update(), WorkerMessage::StartEventLoop]).await;
        timeout(Duration::from_millis(500), entered.notified())
            .await
            .expect("the event future did not start");

        w.worker.exit().await.unwrap();

        assert!(dropped.load(Ordering::SeqCst));
        let saw_idle = w.drain().into_iter().any(|report| {
            matches!(
                report,
                WorkerReport::Status(status)
                    if status.kind == WorkerStatusKind::Activity
                        && status.activity == WorkerActivity::Idle
            )
        });
        assert!(saw_idle, "exit returned before publishing idle");
        assert!(w.worker.send(WorkerMessage::Clear).is_err());
    }

    #[tokio::test]
    async fn exit_cancels_active_execution_before_joining() {
        let entered = Arc::new(Notify::new());
        let observed_cancel = Arc::new(AtomicBool::new(false));

        let mut graph = TestGraph::new();
        graph.add("wait for cancellation", |node| {
            let entered = Arc::clone(&entered);
            let observed_cancel = Arc::clone(&observed_cancel);
            node.sink()
                .lambda(FuncLambda::new(move |Invocation { ctx, .. }| {
                    let cancel = ctx.cancel_flag();
                    let entered = Arc::clone(&entered);
                    let observed_cancel = Arc::clone(&observed_cancel);
                    Box::pin(async move {
                        entered.notify_one();
                        loop {
                            if cancel.is_cancelled() {
                                observed_cancel.store(true, Ordering::SeqCst);
                                return Err(InvokeError::Cancelled);
                            }
                            tokio::task::yield_now().await;
                        }
                    })
                }))
        });

        let mut w = TestWorker::over(graph);
        w.send_many([w.update(), TestWorker::sinks()]);
        timeout(Duration::from_millis(500), entered.notified())
            .await
            .expect("execution did not start");

        timeout(Duration::from_millis(500), w.worker.exit())
            .await
            .expect("the worker did not exit")
            .unwrap();

        assert!(observed_cancel.load(Ordering::SeqCst));
    }

    /// Dropping a `Worker` without `exit` still shuts it down: no callback
    /// fires afterwards. The callback itself is the subject, so this wires a
    /// raw worker.
    #[tokio::test]
    async fn drop_without_exit_shuts_down_cleanly() {
        let reports = Calls::default();
        {
            let counter = reports.clone();
            let worker = Worker::new(move |_| counter.bump());
            let (reply, ack) = oneshot::channel();
            worker.send(WorkerMessage::Sync { reply }).unwrap();
            ack.await.unwrap();
        }

        let before = reports.count();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            before,
            reports.count(),
            "no callback must fire after the Worker is dropped"
        );
    }
}
