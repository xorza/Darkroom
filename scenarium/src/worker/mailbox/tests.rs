use std::sync::Arc;

use crate::exec::compile::Compiler;
use crate::exec::program_event::ProgramEvent;
use crate::graph::graph_set::GraphSet;
use crate::graph::identity::NodeId;
use crate::graph::program_id::ProgramId;
use crate::library::Library;
use crate::worker::batch_intent::{BatchIntent, GraphOp};
use crate::worker::mailbox::{Mailbox, Taken};
use crate::worker::protocol::WorkerMessage;

/// A replaced `Update` lets its program go as the next one merges, before the worker takes
/// the batch: the first program's count falls back to the test's own handle, 1, where a
/// queued message held it, 2, until the worker drained the queue.
#[test]
fn a_replaced_update_drops_its_program_on_merge() {
    let program =
        || Arc::new(Compiler::default().compile(&GraphSet::default(), &Library::default()));
    let (first, second) = (program(), program());
    let mailbox = Mailbox::default();
    mailbox
        .post([WorkerMessage::Update {
            compiled: Arc::clone(&first),
        }])
        .unwrap();
    assert_eq!(Arc::strong_count(&first), 2);
    mailbox
        .post([WorkerMessage::Update {
            compiled: Arc::clone(&second),
        }])
        .unwrap();
    assert_eq!(Arc::strong_count(&first), 1);

    let mut intent = BatchIntent::default();
    assert_eq!(mailbox.take_into(&mut intent), Taken::Batch);
    assert!(
        matches!(&intent.graph_state, Some(GraphOp::Replace(taken)) if Arc::ptr_eq(taken, &second))
    );
}

/// The events a stopped loop fired go, and a post beside them stays: a batch left with a run
/// takes it, one left with nothing is no batch.
#[test]
fn voided_events_leave_what_was_posted_beside_them() {
    let event = ProgramEvent {
        program_id: ProgramId::from(NodeId::from_u128(1)),
        event_idx: 0,
    };
    let mailbox = Mailbox::default();
    let mut intent = BatchIntent::default();

    mailbox.fire(event);
    mailbox.void_events();
    assert_eq!(mailbox.take_into(&mut intent), Taken::Nothing);

    mailbox.fire(event);
    mailbox.post([WorkerMessage::RunSinks]).unwrap();
    mailbox.void_events();
    intent.clear();
    assert_eq!(mailbox.take_into(&mut intent), Taken::Batch);
    assert!(intent.seeds.sinks);
    assert_eq!(intent.seeds.events, []);
}

/// A cancel voids the runs posted before it and cancels the token, and the take that follows
/// clears the token, so a cancel never reaches a run posted after it. A post after the cancel
/// stays.
#[test]
fn a_cancel_voids_the_runs_posted_before_it() {
    let mailbox = Mailbox::default();
    let token = mailbox.cancel_token();
    let mut intent = BatchIntent::default();

    mailbox.post([WorkerMessage::RunSinks]).unwrap();
    mailbox.cancel();
    assert!(token.is_cancelled());
    assert_eq!(mailbox.take_into(&mut intent), Taken::Nothing);
    assert!(!token.is_cancelled());

    mailbox.cancel();
    mailbox.post([WorkerMessage::RunSinks]).unwrap();
    intent.clear();
    assert_eq!(mailbox.take_into(&mut intent), Taken::Batch);
    assert!(intent.seeds.sinks);
    assert!(!token.is_cancelled());
}
