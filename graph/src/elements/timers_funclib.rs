use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::async_lambda;
use crate::data::{DataType, DynamicValue};
use crate::event::EventLambda;
use crate::function::{Func, FuncBehavior, FuncEvent, FuncInput, FuncLib, FuncOutput};
use crate::lambda::FuncLambda;
use crate::prelude::FuncId;
use common::BoolExt;
use common::slot::Slot;
use tokio::sync::Notify;

pub const FRAME_EVENT_FUNC_ID: FuncId = FuncId::from_u128(0x01897c92d6055f5a7a21627ed74824ff);

#[derive(Debug)]
pub struct TimersFuncLib {
    func_lib: FuncLib,

    run_event: Arc<Notify>,
}

#[derive(Debug)]
struct FrameEventCache {
    frame_no: i64,
}

#[derive(Debug, Clone, Copy)]
struct FpsEventState {
    frequency: f64,
    last_execution: Instant,
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

        let fps_state_slot: Slot<FpsEventState> = Slot::default();

        func_lib.add(Func {
            id: FRAME_EVENT_FUNC_ID,
            name: "frame event".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            category: "Timers".to_string(),
            terminal: false,
            inputs: vec![FuncInput {
                name: "frequency".to_string(),
                required: false,
                data_type: DataType::Float,
                default_value: Some((30.0).into()),
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
                    event_lambda: EventLambda::new(|| {
                        Box::pin(async move {
                            //
                        })
                    }),
                },
                FuncEvent {
                    name: "fps".into(),
                    event_lambda: EventLambda::new({
                        let fps_state_slot = fps_state_slot.clone();
                        move || {
                            let fps_state_slot = fps_state_slot.clone();
                            Box::pin(async move {
                                let state = fps_state_slot.peek_or_wait().await;

                                let desired_duration =
                                    Duration::from_secs_f64(1.0 / state.frequency);
                                let elapsed = state.last_execution.elapsed();

                                if elapsed < desired_duration {
                                    tokio::time::sleep(desired_duration - elapsed).await;
                                }
                                // If elapsed >= desired_duration, fire immediately (no sleep)
                            })
                        }
                    }),
                },
            ],
            required_contexts: vec![],
            lambda: async_lambda!(
                move |_context_manager, cache, inputs, _output_usage, outputs| {
                    fps_state_slot = fps_state_slot.clone(),
                } => {
                    let now = Instant::now();

                    let frequency = inputs[0]
                        .value
                        .is_none()
                        .then_else_with(|| 30.0, || inputs[0].value.as_f64());

                    // Get delta from previous state if available
                    let delta = fps_state_slot
                        .take()
                        .map(|state| state.last_execution.elapsed().as_secs_f64())
                        .unwrap_or(1.0 / frequency);

                    // Send new state for the fps event
                    fps_state_slot.send(FpsEventState {
                        frequency,
                        last_execution: now,
                    });

                    let frame_no = {
                        if let Some(frame_event_cache) = cache.get_mut::<FrameEventCache>() {
                            let frame_no = frame_event_cache.frame_no;
                            frame_event_cache.frame_no += 1;
                            frame_no
                        } else {
                            cache.set(FrameEventCache { frame_no: 2 });
                            1
                        }
                    };

                    outputs[0] = DynamicValue::Float(delta);
                    outputs[1] = DynamicValue::Int(frame_no);
                    Ok(())
                }
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
                    move || {
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

impl From<TimersFuncLib> for FuncLib {
    fn from(value: TimersFuncLib) -> Self {
        value.func_lib
    }
}
