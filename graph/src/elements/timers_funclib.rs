use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::data::{DataType, DynamicValue};
use crate::event::EventLambda;
use crate::function::{Func, FuncBehavior, FuncEvent, FuncInput, FuncLib, FuncOutput};
use crate::lambda::FuncLambda;
use crate::prelude::FuncId;
use common::FloatExt;
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

        // Event index for fps event within the frame event func
        const FPS_EVENT_IDX: usize = 1;

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
                    event_lambda: EventLambda::new(|_state| {
                        Box::pin(async move {
                            //
                        })
                    }),
                },
                FuncEvent {
                    name: "fps".into(),
                    event_lambda: EventLambda::new(|state| {
                        Box::pin(async move {
                            // Get current state from per-node event state
                            let fps_state = {
                                let state_guard = state.lock().await;
                                state_guard.get::<FpsEventState>().cloned()
                            };

                            let Some(fps_state) = fps_state else {
                                // No state yet, wait for first execution
                                std::future::pending::<()>().await;
                                return;
                            };

                            if fps_state.frequency.approximately_eq(0.0) {
                                std::future::pending::<()>().await;
                                return;
                            }

                            let desired_duration =
                                Duration::from_secs_f64(1.0 / fps_state.frequency);
                            let elapsed = fps_state.last_execution.elapsed();

                            if elapsed < desired_duration {
                                tokio::time::sleep(desired_duration - elapsed).await;
                            }

                            // If elapsed >= desired_duration, fire immediately (no sleep)
                        })
                    }),
                },
            ],
            required_contexts: vec![],
            lambda: FuncLambda::new(
                move |_context_manager, _state, inputs, _output_usage, outputs, event_states| {
                    Box::pin(async move {
                        let now = Instant::now();

                        let frequency = inputs[0].value.unwrap_or_f64(1.0);

                        // Get previous state from the fps event's state
                        let prev_state = {
                            let state_guard = event_states[FPS_EVENT_IDX].lock().await;
                            state_guard.get::<FpsEventState>().cloned()
                        };

                        let prev_state = prev_state.unwrap_or(FpsEventState {
                            frequency,
                            last_execution: now,
                            frame_no: 0,
                        });

                        let delta = prev_state.last_execution.elapsed().as_secs_f64();
                        let frame_no = prev_state.frame_no + 1;

                        // Store new state in the fps event's state
                        {
                            let mut state_guard = event_states[FPS_EVENT_IDX].lock().await;
                            state_guard.set(FpsEventState {
                                frequency,
                                last_execution: now,
                                frame_no,
                            });
                        }

                        outputs[0] = DynamicValue::Float(delta);
                        outputs[1] = DynamicValue::Int(frame_no);
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
