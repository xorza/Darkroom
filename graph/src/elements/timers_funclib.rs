use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::data::{DataType, DynamicValue};
use crate::event::EventLambda;
use crate::function::{Func, FuncBehavior, FuncEvent, FuncInput, FuncLib, FuncOutput};
use crate::lambda::FuncLambda;
use crate::prelude::FuncId;
use common::FloatExt;
use common::slot::Slot;
use tokio::sync::Notify;

pub const FRAME_EVENT_FUNC_ID: FuncId = FuncId::from_u128(0x01897c92d6055f5a7a21627ed74824ff);

#[derive(Debug)]
pub struct TimersFuncLib {
    func_lib: FuncLib,

    pub run_event: Arc<Notify>,
}

#[derive(Debug, Clone)]
struct FpsEventState {
    frequency: f64,
    last_execution: Instant,
    frame_no: i64,
}

impl TimersFuncLib {
    pub const RUN_FUNC_ID: FuncId = FuncId::from_u128(0xe871ddf47a534ae59728927a88649673);

    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl Default for TimersFuncLib {
    fn default() -> TimersFuncLib {
        let mut func_lib = FuncLib::default();

        func_lib.add(Func {
            id: FRAME_EVENT_FUNC_ID,
            name: "frame event".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            category: "Timers".to_string(),
            terminal: false,
            inputs: vec![FuncInput {
                name: "frequency".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
            }],
            outputs: vec![
                FuncOutput {
                    name: "delta".to_string(),
                    data_type: DataType::Float,
                },
                FuncOutput {
                    name: "frame no".to_string(),
                    data_type: DataType::Int,
                },
            ],
            events: vec![
                FuncEvent {
                    name: "always".into(),
                    event_lambda: EventLambda::new(|state| {
                        Box::pin(async move {
                            // Get current state and increment frame_no
                            let slot = state
                                .lock()
                                .await
                                .get::<Slot<FpsEventState>>()
                                .expect("Node was never executed, nodes should be executed prior to registering events")
                                .clone();
                            let fps_state = slot.peek_or_wait().await;

                            slot.send(FpsEventState {
                                frequency: fps_state.frequency,
                                last_execution: Instant::now(),
                                frame_no: fps_state.frame_no + 1,
                            });
                        })
                    }),
                },
                FuncEvent {
                    name: "fps".into(),
                    event_lambda: EventLambda::new(|state| {
                        Box::pin(async move {
                            // Get current state from per-node event state
                            let slot = state
                                .lock()
                                .await
                                .get::<Slot<FpsEventState>>()
                                .expect("Node was never executed, nodes should be executed prior to registering events")
                                .clone();
                            let fps_state = slot.peek_or_wait().await;

                            if fps_state.frequency.approximately_eq(0.0) {
                                tracing::info!("Frequency is zero, no FPS event");

                                std::future::pending::<()>().await;
                                return;
                            }

                            let desired_duration =
                                Duration::from_secs_f64(1.0 / fps_state.frequency);
                            let elapsed = fps_state.last_execution.elapsed();

                            if elapsed < desired_duration {
                                // todo save last execution time here
                                tokio::time::sleep(desired_duration - elapsed).await;
                            }

                            // update state with new frame number and current time
                            slot.send(FpsEventState {
                                frequency: fps_state.frequency,
                                last_execution: Instant::now(),
                                frame_no: fps_state.frame_no + 1,
                            });

                            // If elapsed >= desired_duration, fire immediately (no sleep)
                        })
                    }),
                },
            ],
            required_contexts: vec![],
            lambda: FuncLambda::new(
                move |_context_manager, _state, event_state, inputs, _output_usage, outputs| {
                    Box::pin(async move {
                        let frequency = inputs[0].value.unwrap_or_f64(1.0);

                        // Get previous state from the event state
                        let slot = event_state
                            .lock()
                            .await
                            .get_or_default_with(|| {
                                let slot = Slot::default();
                                slot.send(FpsEventState {
                                    frequency,
                                    last_execution: Instant::now(),
                                    frame_no: 0,
                                });
                                slot
                            })
                            .clone();

                        let prev_state = slot.peek().unwrap();
                        let delta = prev_state.last_execution.elapsed().as_secs_f64();


                        if !frequency.approximately_eq(prev_state.frequency) {
                            // update frequency
                            slot.send(FpsEventState {
                                frequency,
                                last_execution: prev_state.last_execution,
                                frame_no: prev_state.frame_no,
                            });
                        }

                        outputs[0] = DynamicValue::Float(delta);
                        outputs[1] = DynamicValue::Int(prev_state.frame_no);
                        Ok(())
                    })
                },
            ),
        });

        let run_event = Arc::new(Notify::new());

        func_lib.add(Func {
            id: Self::RUN_FUNC_ID,
            name: "run".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            category: "Timers".to_string(),
            terminal: false,
            inputs: vec![],
            outputs: vec![],
            events: vec![FuncEvent {
                name: "run".into(),
                event_lambda: EventLambda::new({
                    let run_event = Arc::clone(&run_event);
                    move |_state| {
                        let run_event = Arc::clone(&run_event);
                        Box::pin(async move {
                            run_event.notified().await;
                        })
                    }
                }),
            }],
            required_contexts: vec![],
            lambda: FuncLambda::None,
        });

        TimersFuncLib {
            func_lib,
            run_event,
        }
    }
}

impl Default for FpsEventState {
    fn default() -> Self {
        Self {
            frequency: 1.0,
            last_execution: Instant::now(),
            frame_no: 0,
        }
    }
}

impl From<TimersFuncLib> for FuncLib {
    fn from(value: TimersFuncLib) -> Self {
        value.func_lib
    }
}
