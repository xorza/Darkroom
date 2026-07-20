use std::time::{Duration, Instant};

use crate::DataType;
use crate::library::Library;
use crate::node::definition::FuncId;
use crate::node::definition::{Func, FuncInput, FuncOutput};
use crate::node::event::EventLambda;
use crate::node::lambda::FuncLambda;
use common::FloatExt;

pub const FRAME_EVENT_FUNC_ID: FuncId = FuncId::from_u128(0x01897c92d6055f5a7a21627ed74824ff);

#[derive(Debug, Clone)]
struct FpsEventState {
    frequency: f64,
    last_execution: Instant,
    frame_no: i64,
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
                    .default(1.0),
            )
            .output(FuncOutput::new("Delta", DataType::Float).description("Seconds elapsed since the previous frame."))
            .output(FuncOutput::new("Frame #", DataType::Int).description("Frame counter, incremented each tick."))
            .event(
                "Always",
                EventLambda::new(|_state| {
                    Box::pin(async move {
                        //
                    })
                }),
            )
            .event(
                "FPS",
                EventLambda::new(|state| {
                    Box::pin(async move {
                        let fps_state = {
                            let state = state.lock().await;
                            state
                                .get::<FpsEventState>()
                                .expect("Node was never executed, nodes should be executed prior to registering events")
                                .clone()
                        };

                        if fps_state.frequency.approximately_eq(0.0) {
                            tracing::info!("Frequency is zero, no FPS event");

                            std::future::pending::<()>().await;
                            return;
                        }

                        let desired_duration = Duration::from_secs_f64(1.0 / fps_state.frequency);
                        let elapsed = fps_state.last_execution.elapsed();

                        if elapsed < desired_duration {
                            // todo save last execution time here
                            tokio::time::sleep(desired_duration - elapsed).await;
                        }

                        // If elapsed >= desired_duration, fire immediately (no sleep)
                    })
                }),
            )
            .lambda(FuncLambda::new(
                move |_context_manager, _state, event_state, inputs, _output_demand, outputs| {
                    Box::pin(async move {
                        let frequency = inputs[0].value.as_f64().unwrap_or(1.0);
                        let now = Instant::now();

                        let delta;
                        let frame_no;
                        {
                            let mut event_state = event_state.lock().await;
                            let previous = event_state.get_or_default_with(|| FpsEventState {
                                frequency,
                                last_execution: now,
                                frame_no: 1,
                            });
                            let elapsed = previous.last_execution.elapsed().as_secs_f64();
                            delta = if elapsed.approximately_eq(0.0) {
                                1.0 / frequency
                            } else {
                                elapsed
                            };
                            frame_no = previous.frame_no;
                            *previous = FpsEventState {
                                frequency,
                                last_execution: now,
                                frame_no: frame_no + 1,
                            };
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
