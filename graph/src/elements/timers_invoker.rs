use std::str::FromStr;
use std::time::Instant;

use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::FuncBehavior;
use crate::function::{Func, FuncId, FuncInput, FuncLib, FuncOutput};
use crate::invoke::{InvokeArgs, InvokeCache, Invoker, LambdaInvoker};

#[derive(Debug)]
pub struct TimersInvoker {
    lambda_invoker: LambdaInvoker,
}

#[derive(Debug)]
struct FrameEventContext {
    last_frame: Instant,
    frame_no: i64,
}

impl Default for TimersInvoker {
    fn default() -> TimersInvoker {
        let mut invoker = LambdaInvoker::default();

        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c92-d605-5f5a-7a21-627ed74824ff").unwrap(),
                name: "frame event".to_string(),
                description: None,
                behavior: FuncBehavior::Active,
                is_output: false,
                category: "Timers".to_string(),
                inputs: vec![FuncInput {
                    name: "frequency".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(30.0)),
                    variants: vec![],
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
            },
            move |ctx, inputs, outputs| {
                let frequency = inputs[0].as_float();
                let now = Instant::now();

                let (delta, frame_no) = {
                    if let Some(frame_event_ctx) = ctx.get_mut::<FrameEventContext>() {
                        let delta = now.duration_since(frame_event_ctx.last_frame).as_secs_f64();
                        let frame_no = frame_event_ctx.frame_no;

                        frame_event_ctx.last_frame = now;
                        frame_event_ctx.frame_no += 1;

                        (delta, frame_no)
                    } else {
                        ctx.set(FrameEventContext {
                            last_frame: now,
                            frame_no: 2,
                        });

                        (1.0 / frequency, 1)
                    }
                };

                outputs[0] = DynamicValue::Float(delta);
                outputs[1] = DynamicValue::Int(frame_no);
            },
        );

        TimersInvoker {
            lambda_invoker: invoker,
        }
    }
}

impl Invoker for TimersInvoker {
    fn get_func_lib(&mut self) -> FuncLib {
        self.lambda_invoker.get_func_lib()
    }

    fn invoke(
        &self,
        function_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        self.lambda_invoker
            .invoke(function_id, cache, inputs, outputs)
    }
}
