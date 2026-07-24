use std::time::Duration;

use crate::DynamicValue;
use crate::elements::worker_events_library::{FRAME_EVENT_FUNC_ID, worker_events_library};
use crate::node::definition::Func;
use crate::node::lambda::{InvokeError, InvokeInput, InvokeResult, OutputDemand};
use crate::runtime::any_state::AnyState;
use crate::runtime::context::ContextManager;
use crate::runtime::shared_any_state::SharedAnyState;

#[derive(Debug)]
struct FrameOutputs {
    delta: f64,
    frame_no: i64,
}

async fn invoke_frame(
    func: &Func,
    frequency: f64,
    event_state: &SharedAnyState,
) -> InvokeResult<FrameOutputs> {
    let mut context = ContextManager::default();
    let mut state = AnyState::default();
    let mut inputs = [InvokeInput {
        value: frequency.into(),
    }];
    let demand = [OutputDemand::Produce; 2];
    let mut outputs = [DynamicValue::Unbound, DynamicValue::Unbound];

    func.lambda
        .invoke(
            &mut context,
            &mut state,
            event_state,
            &mut inputs,
            &demand,
            &mut outputs,
        )
        .await?;

    Ok(FrameOutputs {
        delta: outputs[0].as_f64().expect("Delta must be a float"),
        frame_no: outputs[1].as_i64().expect("Frame # must be an integer"),
    })
}

fn frame_func() -> Func {
    worker_events_library()
        .by_id(&FRAME_EVENT_FUNC_ID)
        .expect("worker events library must contain Frame Event")
        .clone()
}

#[tokio::test(start_paused = true)]
async fn fps_event_throttles_without_source_reexecution_and_preserves_delta_clock() {
    let func = frame_func();
    assert!(func.inputs[0].const_only);
    let event_state = SharedAnyState::default();
    let initial = invoke_frame(&func, 2.0, &event_state).await.unwrap();
    assert_eq!(initial.delta, 0.5);
    assert_eq!(initial.frame_no, 1);

    for _ in 0..2 {
        let event = func.events[1].event_lambda.clone();
        let tick_state = event_state.clone();
        let tick = tokio::spawn(async move {
            event.invoke(tick_state).await;
        });
        tokio::task::yield_now().await;
        assert!(!tick.is_finished());

        tokio::time::advance(Duration::from_millis(499)).await;
        tokio::task::yield_now().await;
        assert!(!tick.is_finished());

        tokio::time::advance(Duration::from_millis(1)).await;
        tick.await.unwrap();
    }

    let next = invoke_frame(&func, 2.0, &event_state).await.unwrap();
    assert_eq!(next.delta, 1.0);
    assert_eq!(next.frame_no, 2);
}

#[tokio::test(start_paused = true)]
async fn zero_frequency_disables_fps_event_and_starts_with_zero_delta() {
    let func = frame_func();
    let event_state = SharedAnyState::default();
    let initial = invoke_frame(&func, 0.0, &event_state).await.unwrap();
    assert_eq!(initial.delta, 0.0);
    assert_eq!(initial.frame_no, 1);

    let event = func.events[1].event_lambda.clone();
    let tick = tokio::spawn(async move {
        event.invoke(event_state).await;
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(!tick.is_finished());

    tick.abort();
    assert!(tick.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn frame_event_rejects_invalid_frequencies() {
    let func = frame_func();

    for frequency in [
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        f64::MIN_POSITIVE,
    ] {
        let error = invoke_frame(&func, frequency, &SharedAnyState::default())
            .await
            .unwrap_err();
        assert!(
            matches!(error, InvokeError::InvalidInput { index: 0, .. }),
            "unexpected error for {frequency:?}: {error:?}"
        );
    }
}
