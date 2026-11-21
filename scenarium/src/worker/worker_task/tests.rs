use std::time::Instant;

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::identity::program_id::ProgramId;

use crate::exec::report::run_reporter::RunPhase;
use crate::exec::report::run_reporter::RunReporter;
use crate::identity::NodeId;
use crate::worker::batch_intent::{BatchIntent, GraphOp, LoopCommand};
use crate::worker::mailbox::{Mailbox, Taken};
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::worker_task::{EventLoopTransition, ReportSink, WorkerRunReporter, WorkerTask};

fn task_over(mailbox: &Arc<Mailbox>) -> WorkerTask {
    WorkerTask::new(Arc::clone(mailbox), ReportSink::new(|_| {}))
}

/// Every post since the last batch merges into the next one, a wake whose posts were taken
/// returns no empty batch, and a shutdown ends the stream with a post waiting. Once the task
/// drops, a post fails.
#[tokio::test]
async fn next_batch_takes_every_post_since_the_last_batch() {
    let mailbox = Arc::new(Mailbox::default());
    let program_id = ProgramId::from(NodeId::unique());
    mailbox.post([WorkerMessage::Clear]).unwrap();
    mailbox
        .post([WorkerMessage::RunNodes {
            program_ids: vec![program_id],
        }])
        .unwrap();
    let mut task = task_over(&mailbox);

    assert_eq!(task.next_batch().await, Taken::Batch);
    assert!(matches!(task.intent.graph_state, Some(GraphOp::Clear)));
    assert_eq!(task.intent.seeds.program_ids, [program_id]);

    mailbox.post([WorkerMessage::StopEventLoop]).unwrap();
    mailbox
        .post([WorkerMessage::RunNodes {
            program_ids: vec![program_id, program_id],
        }])
        .unwrap();
    assert_eq!(task.next_batch().await, Taken::Batch);
    assert!(matches!(task.intent.loop_request, Some(LoopCommand::Stop)));
    assert!(
        task.intent.graph_state.is_none(),
        "the first batch took the clear"
    );
    assert_eq!(task.intent.seeds.program_ids, [program_id]);

    mailbox.post([WorkerMessage::Clear]).unwrap();
    mailbox.shut_down();
    assert_eq!(task.next_batch().await, Taken::Shutdown);
    drop(task);
    assert!(mailbox.post([WorkerMessage::Clear]).is_err());
}

/// The batches swap buffers rather than allocate them, an applied one included: the third batch
/// gets back the node list the first held, as its run left the list in the batch.
#[tokio::test]
async fn an_applied_batch_hands_its_buffers_back() {
    let mailbox = Arc::new(Mailbox::default());
    let program_id = ProgramId::from(NodeId::unique());
    let run_nodes = || WorkerMessage::RunNodes {
        program_ids: vec![program_id],
    };
    let mut task = task_over(&mailbox);

    mailbox.post([run_nodes()]).unwrap();
    assert_eq!(task.next_batch().await, Taken::Batch);
    let first = task.intent.seeds.program_ids.as_ptr();
    task.apply_intent().await;

    mailbox.post([run_nodes()]).unwrap();
    assert_eq!(task.next_batch().await, Taken::Batch);
    task.apply_intent().await;
    mailbox.post([run_nodes()]).unwrap();
    assert_eq!(task.next_batch().await, Taken::Batch);
    assert_eq!(task.intent.seeds.program_ids.as_ptr(), first);
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

/// Each reported event reaches the host the moment it happens, by value and in order.
#[test]
fn worker_reporter_publishes_each_event_in_order() {
    let first_node = ProgramId::from(NodeId::unique());
    let second_node = ProgramId::from(NodeId::unique());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let reports = ReportSink::new(move |report| tx.send(report).unwrap());
    let mut reporter = WorkerRunReporter { reports: &reports };
    let at = Instant::now();
    reporter.progress(first_node, RunPhase::Started { at });
    reporter.progress(second_node, RunPhase::Failed { elapsed_secs: 0.25 });

    for (node, expected) in [
        (first_node, RunPhase::Started { at }),
        (second_node, RunPhase::Failed { elapsed_secs: 0.25 }),
    ] {
        let WorkerReport::Progress { program_id, phase } = rx.try_recv().unwrap() else {
            panic!("progress must produce a progress report");
        };
        assert_eq!((program_id, phase), (node, expected));
    }
    assert!(rx.try_recv().is_err());
}
