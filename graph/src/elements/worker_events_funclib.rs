use std::time::{Duration, Instant};

use crate::data::{DataType, DynamicValue};
use crate::event_lambda::EventLambda;
use crate::func_lambda::FuncLambda;
use crate::function::{Func, FuncBehavior, FuncEvent, FuncInput, FuncLib, FuncOutput};
use crate::prelude::FuncId;
use common::FloatExt;
use common::slot::Slot;

pub const FRAME_EVENT_FUNC_ID: FuncId = FuncId::from_u128(0x01897c92d6055f5a7a21627ed74824ff);

#[derive(Debug)]
pub struct WorkerEventsFuncLib {
    func_lib: FuncLib,
}

#[derive(Debug, Clone)]
struct FpsEventState {
    frequency: f64,
    last_execution: Instant,
    frame_no: i64,
}

impl WorkerEventsFuncLib {
    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl Default for WorkerEventsFuncLib {
    fn default() -> WorkerEventsFuncLib {
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

                            // If elapsed >= desired_duration, fire immediately (no sleep)
                        })
                    }),
                },
            ],
            required_contexts: vec![],
            lambda: FuncLambda::new(
                move |_context_manager, _state, event_state, inputs, _output_usage, outputs| {
                    Box::pin(async move {
                        let frequency = inputs[0].value.as_f64().unwrap_or(1.0);
                        let now = Instant::now();

                        // Get previous state from the event state
                        let slot = event_state
                            .lock()
                            .await
                            .get_or_default_with(|| {
                                let slot = Slot::default();
                                slot.send(FpsEventState {
                                    frequency,
                                    last_execution: now,
                                    frame_no: 1,
                                });
                                slot
                            })
                            .clone();

                        let prev_state = slot.peek().unwrap();
                        let mut delta = prev_state.last_execution.elapsed().as_secs_f64();
                        if delta.approximately_eq(0.0) {
                            // to avoid
                            delta = 1.0 / frequency;
                        }

                        slot.send(FpsEventState {
                            frequency,
                            last_execution: now,
                            frame_no: prev_state.frame_no + 1,
                        });

                        outputs[0] = DynamicValue::Float(delta);
                        outputs[1] = DynamicValue::Int(prev_state.frame_no);

                        Ok(())
                    })
                },
            ),
        });

        WorkerEventsFuncLib { func_lib }
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

impl From<WorkerEventsFuncLib> for FuncLib {
    fn from(value: WorkerEventsFuncLib) -> Self {
        value.func_lib
    }
}
