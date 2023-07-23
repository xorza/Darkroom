use std::str::FromStr;
use std::time::Instant;

use graph_lib::data::{DataType, DynamicValue, StaticValue};
use graph_lib::function::{Function, FunctionId, InputInfo, OutputInfo};
use graph_lib::graph::FunctionBehavior;
use graph_lib::invoke::{InvokeArgs, Invoker};
use graph_lib::lambda_invoker::LambdaInvoker;
use graph_lib::runtime_graph::InvokeContext;

pub struct TimersInvoker {
    lambda_invoker: LambdaInvoker,
}

impl Default for TimersInvoker {
    fn default() -> TimersInvoker {
        let mut invoker = LambdaInvoker::default();

        invoker.add_lambda(
            Function {
                self_id: FunctionId::from_str("01897c92-d605-5f5a-7a21-627ed74824ff").unwrap(),
                name: "frame event".to_string(),
                behavior: FunctionBehavior::Active,
                is_output: false,
                category: "Timers".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "frequency".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(30.0)),
                        variants: None,
                    }
                ],
                outputs: vec![
                    OutputInfo {
                        name: "delta".to_string(),
                        data_type: DataType::Float,
                    }
                ],
                events: vec![
                    "always".to_string(),
                    "once".to_string(),
                    "fps".to_string(),
                ],
            },
            move |ctx, inputs, outputs| {
                let frequency = inputs[0].as_float();

                let delta =
                    if ctx.is_none() {
                        1.0 / frequency
                    } else {
                        let delta = ctx
                            .get::<Instant>()
                            .unwrap()
                            .elapsed()
                            .as_secs_f64();
                        ctx.set(Instant::now());

                        delta
                    };

                outputs[0] = DynamicValue::Float(delta);
            },
        );

        TimersInvoker {
            lambda_invoker: invoker,
        }
    }
}

impl Invoker for TimersInvoker {
    fn all_functions(&self) -> Vec<Function> {
        self.lambda_invoker.all_functions()
    }

    fn invoke(
        &self,
        function_id: FunctionId,
        ctx: &mut InvokeContext,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        self.lambda_invoker.invoke(function_id, ctx, inputs, outputs)
    }
}