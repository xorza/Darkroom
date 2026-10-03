use super::*;

/// A run's progress reaches the host as it happens: the install first, then
/// the switch to `Executing`, then the node's start and its success, and only
/// then the completion.
#[tokio::test]
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
            WorkerReport::Installed {
                compiled: program,
                cache_ram,
            } => {
                assert!(!installed, "one update installed more than once");
                assert!(Arc::ptr_eq(&program, &compiled));
                assert_eq!(
                    cache_ram,
                    RamUsage::default(),
                    "the first install of a fresh worker reconciles onto an empty cache"
                );
                installed = true;
            }
            WorkerReport::Activity(WorkerActivity::Executing) => {
                assert!(installed, "execution started before installation");
                assert!(!execution_started, "execution started more than once");
                execution_started = true;
            }
            WorkerReport::Progress { node_id, phase } => {
                assert!(execution_started, "node progress arrived before execution");
                assert_eq!(node_id, print, "progress names the node");
                match phase {
                    RunPhase::Started { .. } => started += 1,
                    RunPhase::Succeeded { .. } => node_finished += 1,
                    RunPhase::Failed { .. } => panic!("the node failed"),
                }
            }
            WorkerReport::Completed(summary) => {
                assert!(installed, "completion arrived before installation");
                assert_eq!(summary.activity, WorkerActivity::Idle);
                assert_eq!(started, 1, "one start before completion");
                assert_eq!(node_finished, 1, "one success before completion");
                break;
            }
            WorkerReport::Activity(activity) => panic!("unexpected activity: {activity:?}"),
            WorkerReport::Cleared => panic!("unexpected clear"),
            WorkerReport::Error(error) => panic!("unexpected worker error: {error}"),
        }
    }

    w.send(WorkerMessage::Clear);
    assert!(matches!(w.report().await, WorkerReport::Cleared));
}

/// A → B with trivial sync lambdas, which give the run future no suspension
/// point of their own: nothing but direct reporting can get their progress
/// out mid-run. B records how many progress reports the host callback has
/// already seen — A's start and success *and* B's own start must all have
/// reached the host by the time B's lambda runs.
///
/// The callback itself is the subject here, so this one wires a raw
/// [`Worker`] rather than going through the harness.
#[tokio::test]
async fn live_patches_reach_the_host_before_downstream_nodes_run() {
    let patch_entries = Arc::new(AtomicU64::new(0));
    let seen_by_second = Arc::new(AtomicU64::new(u64::MAX));

    let mut graph = TestGraph::new();
    graph.add("first", |node| {
        node.output(DataType::Int).compute(|_| ConstValue::Int(1))
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
        if let WorkerReport::Progress { .. } = &report {
            entries.fetch_add(1, Ordering::SeqCst);
        }
        #[expect(
            clippy::unused_result_ok,
            reason = "a report sent during teardown has no reader"
        )]
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
        if let WorkerReport::Completed(_) = report {
            break;
        }
    }
    assert_eq!(
        seen_by_second.load(Ordering::SeqCst),
        3,
        "the first node's start and success, and the second's own start, must reach the host \
         before the second node's lambda runs"
    );
}

#[tokio::test]
async fn activity_is_reported_absolutely_and_in_order() {
    let mut w = TestWorker::frames();

    w.settle([w.update(), WorkerMessage::StartEventLoop]).await;

    let mut activities = Vec::new();
    while activities.last() != Some(&WorkerActivity::EventLoop) {
        let activity = w.activity().await;
        if activities.last() != Some(&activity) {
            activities.push(activity);
        }
    }
    assert_eq!(
        activities,
        [WorkerActivity::Executing, WorkerActivity::EventLoop]
    );

    w.settle([WorkerMessage::StopEventLoop]).await;
    while w.activity().await != WorkerActivity::Idle {}
}
