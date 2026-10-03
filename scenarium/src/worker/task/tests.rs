use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use common::CancelToken;

use crate::execution::report::{RunPhase, RunReporter};
use crate::graph::identity::NodeId;
use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand};
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::task::{EventLoopTransition, PendingRun, WorkerRunReporter, WorkerTask};

#[tokio::test]
async fn next_intent_receives_many_messages_into_a_reusable_buffer() {
    let (tx, rx) = mpsc::unbounded_channel();
    let node_id = NodeId::unique();
    tx.send(WorkerMessage::Clear).unwrap();
    tx.send(WorkerMessage::RunNodes {
        nodes: vec![node_id],
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
        [WorkerMessage::RunNodes {
            nodes: vec![node_id],
        }],
        [],
    );
    let run = PendingRun::take(&mut explicit, EventLoopTransition::Preserve).unwrap();
    assert!(!run.start_event_loop);
    assert!(!run.seeds.event_sources);
    assert_eq!(run.seeds.node_ids, [node_id]);
}

/// Each reported event reaches the host the moment it happens, by value and in order.
#[test]
fn worker_reporter_publishes_each_event_in_order() {
    let first_node = NodeId::unique();
    let second_node = NodeId::unique();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let callback = |report| tx.send(report).unwrap();
    let mut reporter = WorkerRunReporter {
        callback: &callback,
    };
    let at = Instant::now();
    reporter.progress(first_node, RunPhase::Started { at });
    reporter.progress(second_node, RunPhase::Failed { elapsed_secs: 0.25 });

    for (node, expected) in [
        (first_node, RunPhase::Started { at }),
        (second_node, RunPhase::Failed { elapsed_secs: 0.25 }),
    ] {
        let WorkerReport::Progress { node_id, phase } = rx.try_recv().unwrap() else {
            panic!("progress must produce a progress report");
        };
        assert_eq!((node_id, phase), (node, expected));
    }
    assert!(rx.try_recv().is_err());
}
