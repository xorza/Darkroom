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
