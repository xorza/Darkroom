use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use ::common::CancelToken;

use crate::execution::report::NodeExecutionStatus;
use crate::execution::report::{RunPhase, RunProgress, RunReporter};
use crate::execution::seeds::RunSeeds;
use crate::graph::identity::NodeId;
use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand};
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::status::{WorkerActivity, WorkerStatusKind, WorkerStatusPublisher};
use crate::worker::task::{EventLoopTransition, PendingRun, WorkerRunReporter, WorkerTask};

#[tokio::test]
async fn next_intent_receives_many_messages_into_a_reusable_buffer() {
    let (tx, rx) = mpsc::unbounded_channel();
    let node_id = NodeId::unique();
    tx.send(WorkerMessage::Clear).unwrap();
    tx.send(WorkerMessage::Run {
        seeds: RunSeeds::nodes(vec![node_id]),
    })
    .unwrap();
    let shutdown = CancellationToken::new();
    let mut task = WorkerTask::new(
        rx,
        |_: WorkerReport| {},
        CancelToken::new(),
        shutdown.clone(),
    );

    {
        let intent = task.next_intent().await.unwrap();
        assert!(matches!(intent.graph_state, Some(GraphOp::Clear)));
        assert_eq!(intent.seeds.node_ids, [node_id]);
    }
    assert!(task.messages.is_empty());
    let capacity = task.messages.capacity();
    assert!(capacity >= 2);

    tx.send(WorkerMessage::StopEventLoop).unwrap();
    let intent = task.next_intent().await.unwrap();
    assert!(matches!(intent.loop_request, Some(LoopCommand::Stop)));
    assert_eq!(task.messages.capacity(), capacity);

    tx.send(WorkerMessage::Clear).unwrap();
    shutdown.cancel();
    assert!(task.next_intent().await.is_none());
}

#[test]
fn event_loop_transition_covers_commands_and_graph_replacement() {
    let cases = [
        (BatchIntent::default(), false, EventLoopTransition::Preserve),
        (BatchIntent::default(), true, EventLoopTransition::Preserve),
        (
            BatchIntent {
                loop_request: Some(LoopCommand::Start),
                ..BatchIntent::default()
            },
            false,
            EventLoopTransition::Rebuild,
        ),
        (
            BatchIntent {
                loop_request: Some(LoopCommand::Start),
                ..BatchIntent::default()
            },
            true,
            EventLoopTransition::Rebuild,
        ),
        (
            BatchIntent {
                loop_request: Some(LoopCommand::Stop),
                ..BatchIntent::default()
            },
            false,
            EventLoopTransition::Stop,
        ),
        (
            BatchIntent {
                loop_request: Some(LoopCommand::Stop),
                ..BatchIntent::default()
            },
            true,
            EventLoopTransition::Stop,
        ),
        (
            BatchIntent {
                graph_state: Some(GraphOp::Clear),
                ..BatchIntent::default()
            },
            false,
            EventLoopTransition::Preserve,
        ),
        (
            BatchIntent {
                graph_state: Some(GraphOp::Clear),
                ..BatchIntent::default()
            },
            true,
            EventLoopTransition::Rebuild,
        ),
        (
            BatchIntent {
                graph_state: Some(GraphOp::Clear),
                loop_request: Some(LoopCommand::Stop),
                ..BatchIntent::default()
            },
            true,
            EventLoopTransition::Stop,
        ),
    ];

    for (intent, active, expected) in cases {
        assert_eq!(EventLoopTransition::for_intent(&intent, active), expected);
    }
}

#[test]
fn pending_run_couples_event_source_initialization_to_loop_rebuild() {
    let mut empty = BatchIntent::default();
    assert!(PendingRun::take(&mut empty, EventLoopTransition::Preserve).is_none());

    let mut rebuild = BatchIntent::default();
    let run = PendingRun::take(&mut rebuild, EventLoopTransition::Rebuild).unwrap();
    assert!(run.start_event_loop);
    assert!(run.seeds.event_sources);
    assert!(!run.seeds.sinks);
    assert!(run.seeds.events.is_empty());
    assert!(run.seeds.node_ids.is_empty());

    let node_id = NodeId::unique();
    let mut explicit = BatchIntent::default();
    explicit.reset(
        [WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![node_id]),
        }],
        [],
    );
    let run = PendingRun::take(&mut explicit, EventLoopTransition::Preserve).unwrap();
    assert!(!run.start_event_loop);
    assert!(!run.seeds.event_sources);
    assert_eq!(run.seeds.node_ids, [node_id]);
}

/// Each reported event publishes its own snapshot the moment it happens, and a snapshot
/// the host has not drained yet is never mutated by the next one.
#[test]
fn worker_reporter_publishes_each_event_and_preserves_published_snapshots() {
    let first_node = NodeId::unique();
    let second_node = NodeId::unique();
    let mut status = WorkerStatusPublisher::default();
    drop(status.activity(WorkerActivity::Executing));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let callback = |report| tx.send(report).unwrap();
    let mut reporter = WorkerRunReporter {
        status: &mut status,
        callback: &callback,
    };

    reporter.progress(RunProgress {
        node_id: first_node,
        phase: RunPhase::Started { at: Instant::now() },
    });
    reporter.progress(RunProgress {
        node_id: second_node,
        phase: RunPhase::Finished { elapsed_secs: 0.25 },
    });

    let WorkerReport::Status(started) = rx.try_recv().unwrap() else {
        panic!("progress must produce a status patch");
    };
    let WorkerReport::Status(finished) = rx.try_recv().unwrap() else {
        panic!("progress must produce a status patch");
    };
    assert!(rx.try_recv().is_err());
    assert_eq!(started.kind, WorkerStatusKind::Patch);
    assert_eq!(started.activity, WorkerActivity::Executing);
    assert_eq!(started.nodes.len(), 1);
    assert_eq!(started.nodes[0].node_id, first_node);
    assert!(matches!(
        started.nodes[0].status,
        Some(NodeExecutionStatus::Running { .. })
    ));
    assert_eq!(finished.nodes.len(), 1);
    assert_eq!(finished.nodes[0].node_id, second_node);
    assert!(matches!(
        finished.nodes[0].status,
        Some(NodeExecutionStatus::Executed { elapsed_secs: 0.25 })
    ));
    // The second patch could not reuse the first's still-queued allocation.
    assert!(!Arc::ptr_eq(&started, &finished));

    // Publishing over a still-queued snapshot allocates fresh rather than deep-cloning
    // vectors it immediately clears — a clone would carry the previous capacity over.
    let idle = status.activity(WorkerActivity::Idle);
    assert!(idle.nodes.is_empty());
    assert_eq!(idle.nodes.capacity(), 0);
    assert_eq!(started.nodes.len(), 1, "a published snapshot is immutable");

    drop((started, finished));
    let allocation = Arc::as_ptr(&idle);
    drop(idle);
    let executing = status.activity(WorkerActivity::Executing);
    assert_eq!(
        Arc::as_ptr(&executing),
        allocation,
        "a drained snapshot's allocation is recycled"
    );
    assert!(rx.try_recv().is_err());
}
