use std::str::FromStr;
use std::time::Instant;

use crate::async_lambda;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::{Func, FuncBehavior, FuncId, FuncInput, FuncLambda, FuncLib, FuncOutput};

#[derive(Debug)]
pub struct TimersInvoker {
    func_lib: FuncLib,
}

#[derive(Debug)]
struct FrameEventCache {
    last_frame: Instant,
    frame_no: i64,
}

impl TimersInvoker {
    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl Default for TimersInvoker {
    fn default() -> TimersInvoker {
        let mut invoker = FuncLib::default();

        invoker.add(Func {
            id: "01897c92-d605-5f5a-7a21-627ed74824ff".into(),
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

                        let frequency = if inputs[0].value.is_none() {
                            30.0
                        } else {
                            inputs[0].value.as_float()
                        };
                        (1.0 / frequency, 1)
                    }
                };

                outputs[0] = DynamicValue::Float(delta);
                outputs[1] = DynamicValue::Int(frame_no);
                Ok(())
            }),
        });

        TimersInvoker { func_lib: invoker }
    }
}
