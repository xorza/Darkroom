use std::str::FromStr;

use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::{Function, FunctionId, InputInfo, OutputInfo};
use crate::graph::FunctionBehavior;
use crate::lambda_invoker::LambdaInvoker;

pub struct TimersInvoker {
    lambda_invoker: LambdaInvoker,
}

impl Default for TimersInvoker {
    fn default() -> TimersInvoker {
        let mut invoker = LambdaInvoker::default();

        invoker.add_lambda(
            Function {
                self_id: FunctionId::from_str("01897c92-d605-5f5a-7a21-627ed74824ff").unwrap(),
                name: "always".to_string(),
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
                    "x times per second".to_string(),
                ],
            },
            move |_ctx, inputs, outputs| {
                let frequency = inputs[0].as_float();
                let delta = 1.0 / frequency;
                outputs[0] = DynamicValue::Float(delta);
                // ctx.set_timer(delta);
            },
        );

        TimersInvoker {
            lambda_invoker: invoker,
        }
    }
}
