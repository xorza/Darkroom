use std::time::Instant;

use crate::async_lambda;
use crate::data::{DataType, DynamicValue};
use crate::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use crate::prelude::{FuncId, FuncLambda};
use common::BoolExt;

pub const FRAME_EVENT_FUNC_ID: FuncId = FuncId::from_u128(0x01897c92d6055f5a7a21627ed74824ff);
pub const RUN_FUNC_ID: FuncId = FuncId::from_u128(0xe871ddf47a534ae59728927a88649673);

#[derive(Debug)]
pub struct TimersFuncLib {
    func_lib: FuncLib,
}

#[derive(Debug)]
struct FrameEventCache {
    last_frame: Instant,
    frame_no: i64,
}

impl TimersFuncLib {
    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl Default for TimersFuncLib {
    fn default() -> TimersFuncLib {
        let mut invoker = FuncLib::default();

        invoker.add(Func {
            id: FRAME_EVENT_FUNC_ID,
            name: "frame event".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            category: "Timers".to_string(),
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
            events: vec!["always".into(), "once".into(), "fps".into()],
            required_contexts: vec![],
            lambda: async_lambda!(move |_context_manager,
                                        cache,
                                        inputs,
                                        _output_usage,
                                        outputs| {
                let now = Instant::now();

                let (delta, frame_no) = {
                    if let Some(frame_event_cache) = cache.get_mut::<FrameEventCache>() {
                        let delta = now
                            .duration_since(frame_event_cache.last_frame)
                            .as_secs_f64();
                        let frame_no = frame_event_cache.frame_no;

                        frame_event_cache.last_frame = now;
                        frame_event_cache.frame_no += 1;

                        (delta, frame_no)
                    } else {
                        cache.set(FrameEventCache {
                            last_frame: now,
                            frame_no: 2,
                        });

                        let frequency = inputs[0]
                            .value
                            .is_none()
                            .then_else_with(|| 30.0, || inputs[0].value.as_f64());
                        (1.0 / frequency, 1)
                    }
                };

                outputs[0] = DynamicValue::Float(delta);
                outputs[1] = DynamicValue::Int(frame_no);
                Ok(())
            }),
        });

        invoker.add(Func {
            id: RUN_FUNC_ID,
            name: "run".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            category: "Timers".to_string(),
            inputs: vec![],
            outputs: vec![],
            events: vec!["run".into()],
            required_contexts: vec![],
            lambda: FuncLambda::None,
        });

        TimersFuncLib { func_lib: invoker }
    }
}

impl From<TimersFuncLib> for FuncLib {
    fn from(value: TimersFuncLib) -> Self {
        value.func_lib
    }
}
