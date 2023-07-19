use std::str::FromStr;

use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};

use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::{Function, FunctionId, InputInfo, OutputInfo};
use crate::graph::FunctionBehavior;
use crate::invoke::{InvokeArgs, Invoker};
use crate::lambda_invoker::LambdaInvoker;
use crate::runtime_graph::InvokeContext;

pub struct BasicInvoker {
    lambda_invoker: LambdaInvoker,
}


#[repr(u32)]
#[derive(Debug, Display, EnumIter, Copy, Clone)]
enum MathTwoArgOp {
    Add = 0,
    Subtract = 1,
    Multiply = 2,
    Divide = 3,
    Modulo = 4,
    Power = 5,
}

impl MathTwoArgOp {
    fn list_variants() -> Vec<(StaticValue, String)> {
        MathTwoArgOp::iter()
            .map(|op| (StaticValue::Int(op as i64), op.to_string()))
            .collect()
    }
    fn invoke(&self, inputs: &InvokeArgs, _context: &InvokeContext) -> anyhow::Result<DynamicValue> {
        assert_eq!(inputs.len(), 2);

        let a = inputs[0].as_ref().unwrap().as_float();
        let b = inputs[1].as_ref().unwrap().as_float();

        Ok(DynamicValue::Float(self.apply(a, b)))
    }
    fn apply(&self, a: f64, b: f64) -> f64 {
        match self {
            MathTwoArgOp::Add => a + b,
            MathTwoArgOp::Subtract => a - b,
            MathTwoArgOp::Multiply => a * b,
            MathTwoArgOp::Divide => a / b,
            MathTwoArgOp::Modulo => a % b,
            MathTwoArgOp::Power => a.powf(b),
        }
    }
}
impl From<MathTwoArgOp> for StaticValue {
    fn from(op: MathTwoArgOp) -> Self {
        StaticValue::Int(op as i64)
    }
}
impl From<i64> for MathTwoArgOp {
    fn from(op: i64) -> Self {
        MathTwoArgOp::iter().find(|op_| *op_ as i64 == op).unwrap()
    }
}

impl Default for BasicInvoker {
    fn default() -> Self {
        let mut invoker = LambdaInvoker::default();

        //print
        invoker.add_lambda(
            Function {
                self_id: FunctionId::from_str("01896910-0790-AD1B-AA12-3F1437196789").unwrap(),
                name: "print".to_string(),
                behavior: FunctionBehavior::Active,
                is_output: true,
                inputs: vec![
                    InputInfo {
                        name: "value".to_string(),
                        is_required: true,
                        data_type: DataType::String,
                        default_value: None,
                        variants: None,
                    },
                ],
                outputs: vec![],
            },
            |_, inputs, _| {
                let value: &str = inputs[0].as_ref().unwrap().as_string();
                println!("{}", value);
            });

        // math two argument operation
        invoker.add_lambda(
            Function {
                self_id: FunctionId::from_str("01896910-4BC9-77AA-6973-64CC1C56B9CE").unwrap(),
                name: "2 arg math".to_string(),
                behavior: FunctionBehavior::Passive,
                is_output: false,
                inputs: vec![
                    InputInfo {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: None,
                        variants: None,
                    },
                    InputInfo {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: None,
                        variants: None,
                    },
                    InputInfo {
                        name: "op".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: Some(StaticValue::from(MathTwoArgOp::Add)),
                        variants: Some(MathTwoArgOp::list_variants()),
                    },
                ],
                outputs: vec![
                    OutputInfo {
                        name: "result".to_string(),
                        data_type: DataType::Float,
                    }
                ],
            },
            move |ctx, inputs, outputs| {
                assert_eq!(inputs.len(), 3);
                assert_eq!(outputs.len(), 1);

                let op: MathTwoArgOp = inputs[2]
                    .as_ref()
                    .unwrap()
                    .as_int()
                    .into();

                op.invoke(&inputs[0..2], ctx)
                    .map(|result| outputs[0] = Some(result))
                    .expect("failed to invoke math two argument operation");
            });

        // to string
        invoker.add_lambda(
            Function {
                self_id: FunctionId::from_str("01896a88-bf15-dead-4a15-5969da5a9e65").unwrap(),
                name: "float to string".to_string(),
                behavior: FunctionBehavior::Passive,
                is_output: false,
                inputs: vec![
                    InputInfo {
                        name: "value".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: None,
                        variants: None,
                    },
                ],
                outputs: vec![
                    OutputInfo {
                        name: "result".to_string(),
                        data_type: DataType::String,
                    }
                ],
            },
            |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let value: f64 = inputs[0].as_ref().unwrap().as_float();
                let result = format!("{}", value);

                outputs[0] = Some(DynamicValue::String(result));
            });

        Self {
            lambda_invoker: invoker,
        }
    }
}

impl Invoker for BasicInvoker {
    fn all_functions(&self) -> Vec<Function> {
        self.lambda_invoker.all_functions()
    }

    fn invoke(&self,
              function_id: FunctionId,
              ctx: &mut InvokeContext,
              inputs: &InvokeArgs,
              outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>
    {
        self.lambda_invoker.invoke(function_id, ctx, inputs, outputs)
    }
}
