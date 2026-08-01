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
        if status.kind == WorkerStatusKind::Activity && status.activity == WorkerActivity::Idle {
            break;
        }
    }
}
