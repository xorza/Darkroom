use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use scenarium::{
    Binding, Compiler, ConstValue, EventPort, ExecError, Graph, InputPort, NodeId, RunSummary,
    WorkerActivity, WorkerError, WorkerReport, system_library, worker_events_library,
};

use crate::core::wake;
use crate::core::wake::Wake;
use crate::core::worker_bridge::{Arrival, WorkerBridge};

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
    graph.subscribe(EventPort::new(frame, 1), print);
    let compiled = Compiler::default().compile(&graph, &library).into();
    bridge
        .install(compiled)
        .expect("worker accepts the program");
    bridge
        .start_event_loop()
        .expect("worker accepts the event-loop start");

    let started_by = Instant::now() + Duration::from_secs(5);
    loop {
        let mut reports = VecDeque::new();
        bridge.drain(&mut reports);
        if reports.into_iter().any(|report| {
            matches!(
                report,
                WorkerReport::Completed(summary) if summary.activity == WorkerActivity::EventLoop
            )
        }) {
            break;
        }
        assert!(
            Instant::now() < started_by,
            "worker did not start its event loop"
        );
        thread::yield_now();
    }
    // A report is posted *before* its wake fires, so seeing one says nothing about its wake
    // having landed. Settling the count against every report posted, merged away or not, makes
    // the drop delta below mean the shutdown wake rather than a straggler from the run. It does
    // settle: the FPS event is disabled at 0 Hz, so its lambda parks forever and no further
    // report is produced until shutdown.
    let delivered = bridge.reports.posts();
    let settle_by = Instant::now() + Duration::from_secs(5);
    while wake_count.load(Ordering::SeqCst) < delivered {
        assert!(
            Instant::now() < settle_by,
            "only {} of {delivered} posted reports woke the host",
            wake_count.load(Ordering::SeqCst),
        );
        thread::yield_now();
    }
    assert_eq!(
        wake_count.load(Ordering::SeqCst),
        delivered,
        "every wake belongs to a report the worker posted"
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
    // A send only fails once the worker task is gone, and then no report can ever arrive.
    let mut bridge = WorkerBridge::new(wake::internals::noop());
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

/// A worker error records its status line and a cancelled run its node count; a run that ends and
/// the other reports record nothing.
#[test]
fn an_arriving_report_records_its_error_and_its_cancel() {
    let node_id = NodeId::from_u128(2);
    let arrivals = [
        WorkerReport::Error(WorkerError::Exec {
            error: ExecError::NodeSeedNotFound { node_id },
        }),
        WorkerReport::Completed(Arc::new(RunSummary {
            cancelled: true,
            executed_node_count: 3,
            ..RunSummary::default()
        })),
        WorkerReport::Completed(Arc::default()),
        WorkerReport::Activity(WorkerActivity::Executing),
        WorkerReport::Cleared,
    ]
    .map(|report| Arrival::of(&report));
    assert_eq!(
        arrivals,
        [
            Some(Arrival::Error(format!(
                "execution failed: node seed {node_id:?} not found in the compiled program"
            ))),
            Some(Arrival::Cancelled {
                executed_node_count: 3
            }),
            None,
            None,
            None,
        ]
    );
}
