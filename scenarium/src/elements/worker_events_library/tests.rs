use std::time::Duration;

use crate::elements::worker_events_library::{FRAME_EVENT_FUNC_ID, worker_events_library};
use crate::graph::func::Func;
use crate::graph::func::error::{InvokeError, InvokeResult};
use crate::testing::func_invoker::FuncInvoker;

#[derive(Debug)]
struct FrameOutputs {
    delta: f64,
    frame_no: i64,
}

/// The frame source, and the invoker holding the event state its FPS event
/// reads. One per test, since a frame counter that restarted between calls
/// would be a different node.
#[derive(Debug)]
struct FrameSource {
    func: Func,
    node: FuncInvoker,
}

impl FrameSource {
    fn new() -> Self {
        Self {
            func: worker_events_library()
                .by_id(FRAME_EVENT_FUNC_ID)
                .expect("worker events library must contain Frame Event")
                .clone(),
            node: FuncInvoker::default(),
        }
    }

    /// Execute the source once at `frequency` Hz.
    async fn tick(&mut self, frequency: f64) -> InvokeResult<FrameOutputs> {
        let outputs = self.node.call(&self.func, [frequency.into()]).await?;
        Ok(FrameOutputs {
            delta: outputs[0].as_f64().expect("Delta must be a float"),
            frame_no: outputs[1].as_i64().expect("Frame # must be an integer"),
        })
    }

    /// A task awaiting the FPS event, over the state this source's executions
    /// write.
    fn fps_event(&self) -> tokio::task::JoinHandle<()> {
        let event = self.func.events[1].event_lambda.clone();
        let state = self.node.event_state();
        tokio::spawn(async move { event.invoke(state).await })
    }
}

#[tokio::test(start_paused = true)]
async fn fps_event_throttles_without_source_reexecution_and_preserves_delta_clock() {
    let mut source = FrameSource::new();
    assert!(source.func.inputs[0].const_only);
    let initial = source.tick(2.0).await.unwrap();
    assert_eq!(initial.delta, 0.5);
    assert_eq!(initial.frame_no, 1);

    for _ in 0..2 {
        let tick = source.fps_event();
        tokio::task::yield_now().await;
        assert!(!tick.is_finished());

        tokio::time::advance(Duration::from_millis(499)).await;
        tokio::task::yield_now().await;
        assert!(!tick.is_finished());

        tokio::time::advance(Duration::from_millis(1)).await;
        tick.await.unwrap();
    }

    let next = source.tick(2.0).await.unwrap();
    assert_eq!(next.delta, 1.0);
    assert_eq!(next.frame_no, 2);
}

/// Re-executing the source must not postpone a waiting FPS event.
///
/// `last_fps_emit` belongs to the event callback, which stamps it when it
/// emits. The lambda also stamping it meant every source execution
/// restarted the countdown, so an `Always` subscriber reading the source's
/// outputs — the ordinary way to consume a frame tick — re-executed it
/// faster than the period and starved the FPS event indefinitely: no
/// error, no missed-deadline signal, just an event that never fires.
///
/// Hand-traced at 2 Hz (500 ms period): the tick is armed at t=0, the
/// source re-executes at t=250 and t=400, and the event must still fire
/// at its original t=500 rather than being pushed out to 750 and 900.
#[tokio::test(start_paused = true)]
async fn source_reexecution_does_not_postpone_a_waiting_fps_event() {
    let mut source = FrameSource::new();
    source.tick(2.0).await.unwrap();

    let tick = source.fps_event();
    tokio::task::yield_now().await;
    assert!(!tick.is_finished());

    // Re-execute the source twice while the event is still waiting.
    for (step, at) in [(250u64, 250u64), (150, 400)] {
        tokio::time::advance(Duration::from_millis(step)).await;
        source.tick(2.0).await.unwrap();
        tokio::task::yield_now().await;
        assert!(!tick.is_finished(), "the event is not due yet at t={at}ms");
    }

    // t=500: the deadline the event was armed for. Asserted *at* that
    // instant rather than by awaiting the handle — `start_paused`
    // auto-advances the clock whenever the runtime goes idle, so a
    // postponed event still fires eventually and only the exact deadline
    // tells the two apart. Yielding (rather than sleeping) keeps the main
    // task runnable, which is what holds the auto-advance off.
    tokio::time::advance(Duration::from_millis(100)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        tick.is_finished(),
        "the FPS event must fire at its own deadline, not one restarted by each execution",
    );
    tick.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn zero_frequency_disables_fps_event_and_starts_with_zero_delta() {
    let mut source = FrameSource::new();
    let initial = source.tick(0.0).await.unwrap();
    assert_eq!(initial.delta, 0.0);
    assert_eq!(initial.frame_no, 1);

    let tick = source.fps_event();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(!tick.is_finished());

    tick.abort();
    assert!(tick.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn frame_event_rejects_invalid_frequencies() {
    for frequency in [
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        f64::MIN_POSITIVE,
    ] {
        // A fresh source each time: the refusal must not depend on history.
        let error = FrameSource::new().tick(frequency).await.unwrap_err();
        assert!(
            matches!(error, InvokeError::InvalidInput { index: 0, .. }),
            "unexpected error for {frequency:?}: {error:?}"
        );
    }
}
