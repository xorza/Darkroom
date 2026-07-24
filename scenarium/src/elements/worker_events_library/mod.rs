use std::time::Duration;

use crate::DataType;
use crate::library::Library;
use crate::node::definition::FuncId;
use crate::node::definition::{Func, FuncInput, FuncOutput};
use crate::node::event::EventLambda;
use crate::node::lambda::{FuncLambda, InvokeError, InvokeResult};
use crate::runtime::shared_any_state::SharedAnyState;
use tokio::time::Instant;

pub const FRAME_EVENT_FUNC_ID: FuncId = FuncId::from_u128(0x01897c92d6055f5a7a21627ed74824ff);

#[derive(Debug)]
struct FpsEventState {
    period: Option<Duration>,
    last_execution: Instant,
    last_fps_emit: Instant,
    frame_no: i64,
}

fn fps_period(frequency: f64) -> InvokeResult<Option<Duration>> {
    const EXPECTED: &str = "zero or a finite positive frequency with a representable period";

    if frequency == 0.0 {
        return Ok(None);
    }
    if !frequency.is_finite() || frequency < 0.0 {
        return Err(InvokeError::invalid_input(0, EXPECTED, frequency));
    }

    let period = Duration::try_from_secs_f64(frequency.recip())
        .map_err(|_| InvokeError::invalid_input(0, EXPECTED, frequency))?;
    if period.is_zero() || Instant::now().checked_add(period).is_none() {
        return Err(InvokeError::invalid_input(0, EXPECTED, frequency));
    }

    Ok(Some(period))
}

async fn wait_for_fps_event(state: SharedAnyState) {
    loop {
        let delay = {
            let state = state.lock().await;
            let fps_state = state
                .get::<FpsEventState>()
                .expect("node must execute before registering its events");
            fps_state
                .period
                .map(|period| period.saturating_sub(fps_state.last_fps_emit.elapsed()))
        };

        let Some(delay) = delay else {
            std::future::pending::<()>().await;
            return;
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        let mut state = state.lock().await;
        let fps_state = state
            .get_mut::<FpsEventState>()
            .expect("node must execute before registering its events");
        let Some(period) = fps_state.period else {
            continue;
        };
        if fps_state.last_fps_emit.elapsed() < period {
            continue;
        }

        fps_state.last_fps_emit = Instant::now();
        return;
    }
}

/// The worker frame / fps event-source nodes.
pub fn worker_events_library() -> Library {
    let mut library = Library::default();

    library.add(
        Func::new(FRAME_EVENT_FUNC_ID, "Frame Event")
            .description("Emits a recurring frame tick, carrying the elapsed time and frame count.")
            .category("System")
            .input(
                FuncInput::required("Frequency", DataType::Float)
                    .description("Target ticks per second (Hz). 0 disables the FPS event.")
                    .default(1.0)
                    .const_only(),
            )
            .output(
                FuncOutput::new("Delta", DataType::Float)
                    .description("Seconds elapsed since the previous frame."),
            )
            .output(
                FuncOutput::new("Frame #", DataType::Int)
                    .description("Frame counter, incremented each tick."),
            )
            .event("Always", EventLambda::new(|_state| Box::pin(async {})))
            .event(
                "FPS",
                EventLambda::new(|state| Box::pin(wait_for_fps_event(state))),
            )
            .lambda(FuncLambda::new(
                move |_context_manager, _state, event_state, inputs, _output_demand, outputs| {
                    Box::pin(async move {
                        let frequency = inputs[0]
                            .value
                            .as_f64()
                            .expect("frequency input type is validated before invocation");
                        let period = fps_period(frequency)?;
                        let now = Instant::now();

                        let delta;
                        let frame_no;
                        {
                            let mut event_state = event_state.lock().await;
                            if let Some(previous) = event_state.get_mut::<FpsEventState>() {
                                delta = previous.last_execution.elapsed().as_secs_f64();
                                frame_no = previous.frame_no;
                                previous.period = period;
                                previous.last_execution = now;
                                previous.last_fps_emit = now;
                                previous.frame_no += 1;
                            } else {
                                delta = period.map_or(0.0, |period| period.as_secs_f64());
                                frame_no = 1;
                                event_state.set(FpsEventState {
                                    period,
                                    last_execution: now,
                                    last_fps_emit: now,
                                    frame_no: 2,
                                });
                            }
                        }

                        outputs[0] = delta.into();
                        outputs[1] = frame_no.into();

                        Ok(())
                    })
                },
            )),
    );

    library
}

#[cfg(test)]
mod tests;
