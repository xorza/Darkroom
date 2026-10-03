use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use scenarium::{
    Binding, Compiler, ConstValue, Graph, InputPort, WorkerActivity, WorkerReport, system_library,
    worker_events_library,
};

use crate::core::wake::Wake;
use crate::core::worker::WorkerBridge;

#[test]
fn drop_waits_for_worker_idle_before_runtime_shutdown() {
    let wake_count = Arc::new(AtomicUsize::new(0));
    let wake: Wake = {
        let wake_count = Arc::clone(&wake_count);
        Arc::new(move || {
            wake_count.fetch_add(1, Ordering::SeqCst);
        })
    };
    let bridge = WorkerBridge::new(wake);

    let mut library = system_library();
    library.merge(worker_events_library());
    let mut graph = Graph::default();
    let frame = graph.add(library.by_name("Frame Event").unwrap().into());
    let print = graph.add(library.by_name("Print").unwrap().into());
    graph.set_input_binding(
        InputPort::new(frame, 0),
        Binding::from(ConstValue::Float(0.0)),
    );
    graph.set_input_binding(
        InputPort::new(print, 0),
        Binding::from(ConstValue::String("tick".to_string())),
    );
    graph.subscribe(frame, 1, print);
    let compiled = Compiler::default()
        .compile(&graph, &library)
        .unwrap()
        .into();
    bridge
        .install(compiled)
        .expect("worker accepts the program");
    bridge
        .start_event_loop()
        .expect("worker accepts the event-loop start");

    let mut delivered = 0_usize;
    loop {
        let report = bridge
            .rx
            .recv_timeout(Duration::from_secs(5))
            .expect("worker did not start its event loop");
        delivered += 1;
        if matches!(
            report,
            WorkerReport::Completed(summary) if summary.activity == WorkerActivity::EventLoop
        ) {
            break;
        }
    }
    // A report is queued *before* its wake fires, so receiving one says
    // nothing about its wake having landed yet. Settling the count against
    // the reports actually taken is what makes the drop delta below mean
    // the shutdown wake rather than a straggler from the run.
    //
    // It does settle: the loop is quiescent — the FPS event is disabled at
    // 0 Hz, so its lambda parks forever and no further report is produced
    // until shutdown.
    let settle_by = Instant::now() + Duration::from_secs(5);
    while wake_count.load(Ordering::SeqCst) < delivered {
        assert!(
            Instant::now() < settle_by,
            "only {} of {delivered} delivered reports woke the host",
            wake_count.load(Ordering::SeqCst),
        );
        thread::yield_now();
    }
    assert_eq!(
        wake_count.load(Ordering::SeqCst),
        delivered,
        "every wake belongs to a report the host was handed"
    );

    drop(bridge);

    assert_eq!(
        wake_count.load(Ordering::SeqCst),
        delivered + 1,
        "shutdown reports the worker idle, and wakes the host for it, before the runtime goes away"
    );
}

#[test]
fn commands_report_a_dead_worker_instead_of_reading_as_queued() {
    // A send only fails once the worker task is gone, and then no
    // report can ever arrive — so a caller told "queued" would wait
    // forever on a run that will never report back.
    let mut bridge = WorkerBridge::new(Arc::new(|| {}));
    assert!(bridge.run_sinks().is_ok(), "a live worker takes commands");

    // Stop the worker the way shutdown does, then keep issuing commands.
    bridge
        .runtime
        .block_on(bridge.worker.exit())
        .expect("worker task joins cleanly");

    assert!(
        bridge.run_sinks().is_err(),
        "a run command to a dead worker must not read as accepted"
    );
    assert!(
        bridge.start_event_loop().is_err(),
        "nor an event-loop start"
    );
}
