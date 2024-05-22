use std::str::FromStr;
use std::sync::Arc;

use log::info;
use parking_lot::Mutex;
use rand::{Rng, SeedableRng};
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};

use common::output_stream::OutputStream;

use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::{Func, FuncId, FuncLib, InputInfo, OutputInfo};
use crate::graph::FuncBehavior;
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
    fn list_variants() -> Vec<(String, StaticValue)> {
        Math2ArgOp::iter()
            .map(|op| (op.to_string(), StaticValue::Int(op as i64)))
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
        Math2ArgOp::iter().find(|op_| *op_ as i64 == op).unwrap()
    }
}

impl BasicInvoker {
    pub(crate) fn use_output_stream(&mut self, output_stream: &OutputStream) {
        self.output_stream.lock().replace(output_stream.clone());
    }
}

impl Default for BasicInvoker {
    fn default() -> Self {
        let mut invoker = LambdaInvoker::default();
        let output_stream = Arc::new(Mutex::new(None::<OutputStream>));
        let output_stream_clone = output_stream.clone();

        //print
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01896910-0790-AD1B-AA12-3F1437196789").unwrap(),
                name: "print".to_string(),
                behavior: FuncBehavior::Active,
                is_output: true,
                category: "math".to_string(),
                inputs: vec![InputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01896910-4BC9-77AA-6973-64CC1C56B9CE").unwrap(),
                name: "2 arg math".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: None,
                        variants: vec![],
                    },
                    InputInfo {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: None,
                        variants: vec![],
                    },
                    InputInfo {
                        name: "op".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: Some(StaticValue::from(Math2ArgOp::Add)),
                        variants: Math2ArgOp::list_variants(),
                    },
                ],
                outputs: vec![OutputInfo {
                    name: "result".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01896a88-bf15-dead-4a15-5969da5a9e65").unwrap(),
                name: "float to string".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "value".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: None,
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897928-66cd-52cb-abeb-a5bfd7f3763e").unwrap(),
                name: "random".to_string(),
                behavior: FuncBehavior::Active,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "min".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    InputInfo {
                        name: "max".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
                    name: "result".to_string(),
                    data_type: DataType::Float,
                }],
                events: vec![],
            },
            move |cache, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);


                let rng = cache.get_or_default_with(rand::rngs::StdRng::from_entropy);

                let min: f64 = inputs[0].as_float();
                let max: f64 = inputs[1].as_float();
                let random = rng.gen::<f64>();
                let result = min + (max - min) * random;

                outputs[0] = DynamicValue::Float(result);
            },
        );
        //add
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c4c-ac6a-84c0-d0b7-17d49e1ae2ee").unwrap(),
                name: "add".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    InputInfo {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c50-229e-f5e4-1c60-7f1e14531da2").unwrap(),
                name: "subtract".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    InputInfo {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c50-d510-55bf-8cb9-545a62cc76cc").unwrap(),
                name: "multiply".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    InputInfo {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c50-2b4e-4f0e-8f0a-5b0b8b2b4b4b").unwrap(),
                name: "divide".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    InputInfo {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![
                    OutputInfo {
                        name: "divide".to_string(),
                        data_type: DataType::Float,
                    },
                    OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c52-ac50-733e-aeeb-7018fd84c264").unwrap(),
                name: "power".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "a".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(0.0)),
                        variants: vec![],
                    },
                    InputInfo {
                        name: "b".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c53-a3d7-e716-b80a-0ba98661413a").unwrap(),
                name: "sqrt".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c54-8671-5d7c-db4c-aca72865a5a6").unwrap(),
                name: "sin".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c54-ceb5-e603-ebde-c6904a8ef6e5").unwrap(),
                name: "cos".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-1fda-2837-f4bd-75bea812a70e").unwrap(),
                name: "tan".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "a".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-6920-1641-593c-5a1d91c033cb").unwrap(),
                name: "asin".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "sin".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-a3ef-681e-6fbb-5133c96f720c").unwrap(),
                name: "acos".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "cos".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(1.0)),
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c55-e6f4-726c-5d4e-a2f90c4fc43b").unwrap(),
                name: "atan".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![InputInfo {
                    name: "tan".to_string(),
                    is_required: true,
                    data_type: DataType::Float,
                    default_value: Some(StaticValue::Float(0.0)),
                    variants: vec![],
                }],
                outputs: vec![OutputInfo {
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
        invoker.add_lambda(
            Func {
                id: FuncId::from_str("01897c56-8dde-c5f3-a389-f326fdf81b3a").unwrap(),
                name: "log".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                category: "math".to_string(),
                inputs: vec![
                    InputInfo {
                        name: "value".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(1.0)),
                        variants: vec![],
                    },
                    InputInfo {
                        name: "base".to_string(),
                        is_required: true,
                        data_type: DataType::Float,
                        default_value: Some(StaticValue::Float(10.0)),
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
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
            lambda_invoker: invoker,
            output_stream,
        }
    }
}

impl Invoker for BasicInvoker {
    fn take_func_lib(&mut self) -> FuncLib {
        self.lambda_invoker.take_func_lib()
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
