use std::str::FromStr;
use std::sync::Arc;

use parking_lot::Mutex;
use rand::{Rng, SeedableRng};
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};
use tracing::info;

use common::output_stream::OutputStream;

use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::FuncBehavior;
use crate::function::{Func, FuncId, FuncInput, FuncLib, FuncOutput, ValueVariant};
use crate::invoke::{InvokeArgs, InvokeCache, Invoker, LambdaInvoker};

#[derive(Debug)]
pub struct BasicInvoker {
    lambda_invoker: LambdaInvoker,
    output_stream: Arc<Mutex<Option<OutputStream>>>,
}

#[repr(u32)]
#[derive(Debug, Display, EnumIter, Copy, Clone)]
enum Math2ArgOp {
    Add = 0,
    Subtract = 1,
    Multiply = 2,
    Divide = 3,
    Modulo = 4,
    Power = 5,
    Log = 6,
}

impl Math2ArgOp {
    fn list_variants() -> Vec<ValueVariant> {
        Math2ArgOp::iter()
            .map(|op| ValueVariant {
                name: op.to_string(),
                value: StaticValue::Int(op as i64),
            })
            .collect()
    }
    fn invoke(&self, inputs: &InvokeArgs) -> anyhow::Result<DynamicValue> {
        assert_eq!(inputs.len(), 2);

        let a = inputs[0].as_float();
        let b = inputs[1].as_float();

        Ok(DynamicValue::Float(self.apply(a, b)))
    }
    fn apply(&self, a: f64, b: f64) -> f64 {
        match self {
            Math2ArgOp::Add => a + b,
            Math2ArgOp::Subtract => a - b,
            Math2ArgOp::Multiply => a * b,
            Math2ArgOp::Divide => a / b,
            Math2ArgOp::Modulo => a % b,
            Math2ArgOp::Power => a.powf(b),
            Math2ArgOp::Log => a.log(b),
        }
    }
}

impl From<Math2ArgOp> for StaticValue {
    fn from(op: Math2ArgOp) -> Self {
        StaticValue::Int(op as i64)
    }
}

impl From<i64> for Math2ArgOp {
    fn from(op: i64) -> Self {
        Math2ArgOp::iter()
            .find(|op_| *op_ as i64 == op)
            .expect("Unknown math op")
    }
}

impl BasicInvoker {
    pub(crate) fn use_output_stream(&mut self, output_stream: &OutputStream) {
        self.output_stream.lock().replace(output_stream.clone());
    }
}

impl Default for BasicInvoker {
    fn default() -> Self {
        let mut lambda_invoker = LambdaInvoker::default();
        let output_stream = Arc::new(Mutex::new(None::<OutputStream>));
        let output_stream_clone = output_stream.clone();

        //print, outputs to output_stream
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01896910-0790-AD1B-AA12-3F1437196789")
                    .expect("Invalid func id"),
                name: "print".to_string(),
                description: None,
                behavior: FuncBehavior::Active,
                is_output: true,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "value".to_string(),
                    is_required: true,
                    data_type: DataType::String,
                    default_value: None,
                    variants: vec![],
                }],
                outputs: vec![],
                events: vec![],
            },
            move |_, inputs, _| {
                let value: &str = inputs[0].as_string();
                let _ = output_stream_clone.lock().as_mut().is_some_and(|s| {
                    s.write(value);
                    true
                });
                info!("{:?}", value);
            },
        );
        // math two argument operation
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01896910-4BC9-77AA-6973-64CC1C56B9CE")
                    .expect("Invalid func id"),
                name: "2 arg math".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: None,
                        variants: vec![],
                    },
                    FuncInput {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: None,
                        variants: vec![],
                    },
                    FuncInput {
                        name: "op".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: Some(StaticValue::from(Math2ArgOp::Add)),
                        variants: Math2ArgOp::list_variants(),
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "result".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
                description: None,
            },
            move |_cache, inputs, outputs| {
                assert_eq!(inputs.len(), 3);
                assert_eq!(outputs.len(), 1);

                let op: Math2ArgOp = inputs[2].as_int().into();

                op.invoke(&inputs[0..2])
                    .map(|result| outputs[0] = result)
                    .expect("failed to invoke math two argument operation");
            },
        );
        // to string
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01896a88-bf15-dead-4a15-5969da5a9e65")
                    .expect("Invalid func id"),
                name: "float to string".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "value".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: None,
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "result".to_string(),
                    data_type: DataType::String,
                }],
                events: vec![],
            },
            |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let value: f64 = inputs[0].as_float();
                let result = format!("{}", value);

                outputs[0] = DynamicValue::String(result);
            },
        );

        // random
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897928-66cd-52cb-abeb-a5bfd7f3763e")
                    .expect("Invalid func id"),
                name: "random".to_string(),
                description: None,
                behavior: FuncBehavior::Active,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "min".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    FuncInput {
                        name: "max".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "result".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |cache, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let rng = cache.get_or_default_with(rand::rngs::StdRng::from_os_rng);

                let min: f64 = inputs[0].as_float();
                let max: f64 = inputs[1].as_float();
                let random = rng.random::<f64>();
                let result = min + (max - min) * random;

                outputs[0] = DynamicValue::Float(result);
            },
        );
        //add
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c4c-ac6a-84c0-d0b7-17d49e1ae2ee")
                    .expect("Invalid func id"),
                name: "add".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    FuncInput {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "result".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let b: f64 = inputs[1].as_float();
                let result = a + b;

                outputs[0] = DynamicValue::Float(result);
            },
        );
        //subtract
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c50-229e-f5e4-1c60-7f1e14531da2")
                    .expect("Invalid func id"),
                name: "subtract".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    FuncInput {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "result".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let b: f64 = inputs[1].as_float();
                let result = a - b;

                outputs[0] = DynamicValue::Float(result);
            },
        );
        //multiply
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c50-d510-55bf-8cb9-545a62cc76cc")
                    .expect("Invalid func id"),
                name: "multiply".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    FuncInput {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "result".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let b: f64 = inputs[1].as_float();
                let result = a * b;

                outputs[0] = DynamicValue::Float(result);
            },
        );
        //divide
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c50-2b4e-4f0e-8f0a-5b0b8b2b4b4b")
                    .expect("Invalid func id"),
                name: "divide".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    FuncInput {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![
                    FuncOutput {
                        name: "divide".to_string(),
                        data_type: DataType::Float,
                    },
                    FuncOutput {
                        name: "modulo".to_string(),
                        data_type: DataType::Float,
                    },
                ],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let b: f64 = inputs[1].as_float();
                let divide = a / b;
                let modulo = a % b;

                outputs[0] = DynamicValue::Float(divide);
                outputs[1] = DynamicValue::Float(modulo);
            },
        );
        // power
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c52-ac50-733e-aeeb-7018fd84c264")
                    .expect("Invalid func id"),
                name: "power".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    FuncInput {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "power".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let b: f64 = inputs[1].as_float();
                let power = a.powf(b);

                outputs[0] = DynamicValue::Float(power);
            },
        );
        // sqrt
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c53-a3d7-e716-b80a-0ba98661413a")
                    .expect("Invalid func id"),
                name: "sqrt".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "sqrt".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let sqrt = a.sqrt();

                outputs[0] = DynamicValue::Float(sqrt);
            },
        );
        // sin
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c54-8671-5d7c-db4c-aca72865a5a6")
                    .expect("Invalid func id"),
                name: "sin".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "sin".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let sin = a.sin();

                outputs[0] = DynamicValue::Float(sin);
            },
        );
        // cos
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c54-ceb5-e603-ebde-c6904a8ef6e5")
                    .expect("Invalid func id"),
                name: "cos".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "cos".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let cos = a.cos();

                outputs[0] = DynamicValue::Float(cos);
            },
        );
        // tan
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-1fda-2837-f4bd-75bea812a70e")
                    .expect("Invalid func id"),
                name: "tan".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "tan".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = inputs[0].as_float();
                let tan = a.tan();

                outputs[0] = DynamicValue::Float(tan);
            },
        );
        // asin
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-6920-1641-593c-5a1d91c033cb")
                    .expect("Invalid func id"),
                name: "asin".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "sin".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "asin".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let sin: f64 = inputs[0].as_float();
                let asin = sin.asin();

                outputs[0] = DynamicValue::Float(asin);
            },
        );
        // acos
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-a3ef-681e-6fbb-5133c96f720c")
                    .expect("Invalid func id"),
                name: "acos".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "cos".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(1.0)),
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "acos".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let cos: f64 = inputs[0].as_float();
                let acos = cos.acos();

                outputs[0] = DynamicValue::Float(acos);
            },
        );
        // atan
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-e6f4-726c-5d4e-a2f90c4fc43b")
                    .expect("Invalid func id"),
                name: "atan".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![FuncInput {
                    name: "tan".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![FuncOutput {
                    name: "atan".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let tan: f64 = inputs[0].as_float();
                let atan = tan.atan();

                outputs[0] = DynamicValue::Float(atan);
            },
        );
        // log
        lambda_invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c56-8dde-c5f3-a389-f326fdf81b3a")
                    .expect("Invalid func id"),
                name: "log".to_string(),
                description: None,
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    FuncInput {
                        name: "value".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                    FuncInput {
                        name: "base".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(10.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "log".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |_, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let value: f64 = inputs[0].as_float();
                let base: f64 = inputs[1].as_float();
                let log = value.log(base);

                outputs[0] = DynamicValue::Float(log);
            },
        );

        Self {
            lambda_invoker,
            output_stream,
        }
    }
}

impl Invoker for BasicInvoker {
    fn get_func_lib(&self) -> FuncLib {
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
