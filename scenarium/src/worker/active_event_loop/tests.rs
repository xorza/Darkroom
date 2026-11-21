use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;
use tokio::time;
use tokio::time::{Duration, timeout};

use crate::decl::func::event_lambda::EventLambda;
use crate::exec::program_event::ProgramEvent;
use crate::identity::NodeId;
use crate::identity::program_id::ProgramId;
use crate::internals::worker::PATIENCE;
use crate::runtime::shared_any_state::SharedAnyState;
use crate::worker::batch_intent::BatchIntent;
use crate::worker::mailbox::Taken;

/// A loop over one event, the mailbox it fires into and the count that paces it.
#[derive(Debug)]
struct SingleLoop {
    active: ActiveEventLoop,
    mailbox: Arc<Mailbox>,
    epoch: RunEpoch,
    event: ProgramEvent,
}

fn start_single_event_loop(lambda: EventLambda) -> SingleLoop {
    let mailbox = Arc::new(Mailbox::default());
    let epoch = RunEpoch::default();
    let event = ProgramEvent {
        program_id: ProgramId::from(NodeId::unique()),
        event_idx: 0,
    };
    let active = ActiveEventLoop::start(
        vec![ArmedEvent {
            event,
            lambda,
            state: SharedAnyState::default(),
        }],
        &mailbox,
        &epoch,
    );
    SingleLoop {
        active,
        mailbox,
        epoch,
        event,
    }
}

/// The fired events the mailbox holds.
fn fired(mailbox: &Mailbox) -> Vec<ProgramEvent> {
    let mut intent = BatchIntent::default();
    match mailbox.take_into(&mut intent) {
        Taken::Nothing => Vec::new(),
        Taken::Batch => intent.seeds.events,
        Taken::Shutdown => panic!("nothing shut the mailbox down"),
    }
}

/// Under paused time, a sleep returns only once every task has run to its next block point.
async fn settle() {
    time::sleep(Duration::from_millis(1)).await;
}

/// A fire reaches the mailbox once the lambda's future completes: none while it waits, the event
/// once it is let go.
#[tokio::test(start_paused = true)]
async fn a_fire_reaches_the_mailbox_when_the_lambda_completes() {
    let proceed = Arc::new(Notify::new());
    let lambda = EventLambda::new({
        let proceed = Arc::clone(&proceed);
        move |_state| {
            let proceed = Arc::clone(&proceed);
            async move { proceed.notified().await }
        }
    });
    let SingleLoop {
        mut active,
        mailbox,
        event,
        ..
    } = start_single_event_loop(lambda);

    settle().await;
    assert!(fired(&mailbox).is_empty());

    proceed.notify_one();
    timeout(PATIENCE, mailbox.posted()).await.unwrap();
    assert_eq!(fired(&mailbox), [event]);

    active.stop().await;
}

/// A source fires once per finished batch however fast its stream yields: an always-ready lambda
/// runs once, then waits, and each advance of the count lets it run once more.
#[tokio::test(start_paused = true)]
async fn a_source_fires_once_per_finished_batch() {
    let invocations = Arc::new(AtomicUsize::new(0));
    let lambda = EventLambda::new({
        let invocations = Arc::clone(&invocations);
        move |_state| {
            invocations.fetch_add(1, Ordering::SeqCst);
            async {}
        }
    });
    let SingleLoop {
        mut active,
        mailbox,
        epoch,
        event,
    } = start_single_event_loop(lambda);

    settle().await;
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(fired(&mailbox), [event]);

    settle().await;
    assert_eq!(invocations.load(Ordering::SeqCst), 1, "no batch finished");

    epoch.advance();
    settle().await;
    assert_eq!(invocations.load(Ordering::SeqCst), 2);
    assert_eq!(fired(&mailbox), [event]);

    active.stop().await;
}

/// A lambda's panic is resumed by `failed`, on whatever awaits it.
#[tokio::test]
async fn a_lambda_panic_is_resumed_by_failed() {
    let lambda = EventLambda::new(|_state| async { panic!("boom in lambda") });
    let SingleLoop { mut active, .. } = start_single_event_loop(lambda);

    let error = tokio::spawn(async move { active.failed().await })
        .await
        .expect_err("the panic is resumed");
    let payload = error.into_panic();
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"boom in lambda"));
}

/// Stopping joins every task, so none holds the mailbox afterwards: the test's handle is its one.
#[tokio::test]
async fn a_stopped_loop_holds_nothing() {
    let lambda = EventLambda::new(|_state| async {});
    let SingleLoop {
        mut active,
        mailbox,
        ..
    } = start_single_event_loop(lambda);

    active.stop().await;
    assert_eq!(Arc::strong_count(&mailbox), 1);
}
