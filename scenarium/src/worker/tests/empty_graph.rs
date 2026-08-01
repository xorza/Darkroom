//! Running on a worker holding no graph is an ordinary state, not a failure:
//! the worker skips execution silently and publishes nothing at all.

use super::*;

/// Every way of asking a graphless worker to do something — sink seeds, an
/// event seed naming a node it has never held, starting the event loop, and
/// the two combined in one batch — is silently nothing.
#[tokio::test]
async fn every_seed_is_a_silent_noop() {
    let stale_event = || WorkerMessage::Run {
        seeds: RunSeeds::events(vec![EventPort {
            node_id: NodeId::unique(),
            event_idx: 0,
        }]),
    };
    let batches: [Vec<WorkerMessage>; 4] = [
        vec![TestWorker::sinks()],
        vec![stale_event()],
        vec![WorkerMessage::StartEventLoop],
        vec![TestWorker::sinks(), WorkerMessage::StartEventLoop],
    ];

    for batch in batches {
        let mut w = TestWorker::over(TestGraph::new());

        w.settle(batch).await;

        w.nothing_runs_within(QUIET).await;
    }
}
