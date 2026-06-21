use rand::{RngExt, SeedableRng};
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};

use crate::async_lambda;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::func_lambda::InvokeInput;
use crate::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput, ValueOption};

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
    fn list_variants() -> Vec<ValueOption> {
        Math2ArgOp::iter()
            .map(|op| ValueOption {
                name: op.to_string(),
                value: StaticValue::Int(op as i64),
            })
            .collect()
    }
    fn invoke(&self, inputs: &[InvokeInput]) -> anyhow::Result<DynamicValue> {
        assert_eq!(inputs.len(), 2);

        let a = inputs[0].value.as_f64().unwrap();
        let b = inputs[1].value.as_f64().unwrap();

        Ok(self.apply(a, b).into())
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

/// The built-in math / string / print nodes.
pub fn basic_funclib() -> FuncLib {
    let mut func_lib = FuncLib::default();

    // print: log the input string to the node log (info level), read
    // back by the editor. Sugar over `ContextManager::log`.
    func_lib.add(Func {
        id: "01896910-0790-AD1B-AA12-3F1437196789".into(),
        name: "print".to_string(),
        description: Some("Logs a string value to the node log".to_string()),
        behavior: FuncBehavior::Impure,
        terminal: true,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "value".to_string(),
            required: true,
            data_type: DataType::String,
            default_value: None,
            value_options: vec![],
        }],
        outputs: vec![],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |ctx, _, _, inputs, _, _| {
            assert_eq!(inputs.len(), 1);
            let value: &str = inputs[0].value.as_string().unwrap();
            ctx.info(value);
            Ok(())
        }),
        ..Default::default()
    });
    // math two argument operation
    func_lib.add(Func {
            id: "01896910-4BC9-77AA-6973-64CC1C56B9CE".into(),
            name: "2 arg math".to_string(),
            description: Some("Performs a two-argument math operation (add, subtract, multiply, divide, modulo, power, log)".to_string()),
            behavior: FuncBehavior::Pure,
            terminal: false,
            category: "math".to_string(),
            inputs: vec![
                FuncInput {
                    name: "a".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "b".to_string(),
                    required: true,
                    data_type: DataType::Float,
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "op".to_string(),
                    required: true,
                    data_type: DataType::Int,
                    default_value: Some((Math2ArgOp::Add).into()),
                    value_options: Math2ArgOp::list_variants(),
                },
            ],
            outputs: vec![FuncOutput {
                name: "result".to_string(),
                data_type: DataType::Float,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 3);
                assert_eq!(outputs.len(), 1);

                let op: Math2ArgOp = inputs[2].value.as_i64().unwrap().into();

                op.invoke(&inputs[0..2])
                    .map(|result| outputs[0] = result)
                    .expect("failed to invoke math two argument operation");
                Ok(())
            }),
            ..Default::default()
        });
    // to string
    func_lib.add(Func {
        id: "01896a88-bf15-dead-4a15-5969da5a9e65".into(),
        name: "float to string".to_string(),
        description: Some("Converts a float value to its string representation".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "value".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: None,
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "result".to_string(),
            data_type: DataType::String,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(|_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let value: f64 = inputs[0].value.as_f64().unwrap();
            let result = value.to_string();

            outputs[0] = result.into();
            Ok(())
        }),
        ..Default::default()
    });

    // random
    func_lib.add(Func {
        id: "01897928-66cd-52cb-abeb-a5bfd7f3763e".into(),
        name: "random".to_string(),
        description: Some("Generates a random float between min and max values".to_string()),
        behavior: FuncBehavior::Impure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![
            FuncInput {
                name: "min".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((0.0).into()),
                value_options: vec![],
            },
            FuncInput {
                name: "max".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
            },
        ],
        outputs: vec![FuncOutput {
            name: "result".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, cache, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);

            let rng = cache.get_or_default_with(|| rand::rngs::StdRng::from_rng(&mut rand::rng()));

            let min: f64 = inputs[0].value.as_f64().unwrap();
            let max: f64 = inputs[1].value.as_f64().unwrap();
            let random = rng.random::<f64>();
            let result = min + (max - min) * random;

            outputs[0] = result.into();
            Ok(())
        }),
        ..Default::default()
    });
    //add
    func_lib.add(Func {
        id: "01897c4c-ac6a-84c0-d0b7-17d49e1ae2ee".into(),
        name: "add".to_string(),
        description: Some("Adds two float values (a + b)".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![
            FuncInput {
                name: "a".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((0.0).into()),
                value_options: vec![],
            },
            FuncInput {
                name: "b".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
            },
        ],
        outputs: vec![FuncOutput {
            name: "result".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let b: f64 = inputs[1].value.as_f64().unwrap();
            let result = a + b;

            outputs[0] = result.into();
            Ok(())
        }),
        ..Default::default()
    });
    //subtract
    func_lib.add(Func {
        id: "01897c50-229e-f5e4-1c60-7f1e14531da2".into(),
        name: "subtract".to_string(),
        description: Some("Subtracts the second value from the first (a - b)".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![
            FuncInput {
                name: "a".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((0.0).into()),
                value_options: vec![],
            },
            FuncInput {
                name: "b".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
            },
        ],
        outputs: vec![FuncOutput {
            name: "result".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let b: f64 = inputs[1].value.as_f64().unwrap();
            let result = a - b;

            outputs[0] = result.into();
            Ok(())
        }),
        ..Default::default()
    });
    //multiply
    func_lib.add(Func {
        id: "01897c50-d510-55bf-8cb9-545a62cc76cc".into(),
        name: "multiply".to_string(),
        description: Some("Multiplies two float values (a * b)".to_string()),
        behavior: FuncBehavior::Pure,
        category: "math".to_string(),
        terminal: false,
        inputs: vec![
            FuncInput {
                name: "a".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((0.0).into()),
                value_options: vec![],
            },
            FuncInput {
                name: "b".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
            },
        ],
        outputs: vec![FuncOutput {
            name: "result".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let b: f64 = inputs[1].value.as_f64().unwrap();
            let result = a * b;

            outputs[0] = result.into();
            Ok(())
        }),
        ..Default::default()
    });
    //divide
    func_lib.add(Func {
        id: "01897c50-2b4e-4f0e-8f0a-5b0b8b2b4b4b".into(),
        name: "divide".to_string(),
        description: Some(
            "Divides the first value by the second, outputs both quotient and modulo".to_string(),
        ),
        behavior: FuncBehavior::Pure,
        category: "math".to_string(),
        terminal: false,
        inputs: vec![
            FuncInput {
                name: "a".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((0.0).into()),
                value_options: vec![],
            },
            FuncInput {
                name: "b".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
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
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 2);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let b: f64 = inputs[1].value.as_f64().unwrap();
            let divide = a / b;
            let modulo = a % b;

            outputs[0] = divide.into();
            outputs[1] = modulo.into();
            Ok(())
        }),
        ..Default::default()
    });
    // power
    func_lib.add(Func {
        id: "01897c52-ac50-733e-aeeb-7018fd84c264".into(),
        name: "power".to_string(),
        description: Some("Raises the first value to the power of the second (a^b)".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![
            FuncInput {
                name: "a".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((0.0).into()),
                value_options: vec![],
            },
            FuncInput {
                name: "b".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
            },
        ],
        outputs: vec![FuncOutput {
            name: "power".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let b: f64 = inputs[1].value.as_f64().unwrap();
            let power = a.powf(b);

            outputs[0] = power.into();
            Ok(())
        }),
        ..Default::default()
    });
    // sqrt
    func_lib.add(Func {
        id: "01897c53-a3d7-e716-b80a-0ba98661413a".into(),
        name: "sqrt".to_string(),
        description: Some("Calculates the square root of a value".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "a".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: Some((0.0).into()),
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "sqrt".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let sqrt = a.sqrt();

            outputs[0] = sqrt.into();
            Ok(())
        }),
        ..Default::default()
    });
    // sin
    func_lib.add(Func {
        id: "01897c54-8671-5d7c-db4c-aca72865a5a6".into(),
        name: "sin".to_string(),
        description: Some("Calculates the sine of an angle in radians".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "a".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: Some((0.0).into()),
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "sin".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let sin = a.sin();

            outputs[0] = sin.into();
            Ok(())
        }),
        ..Default::default()
    });
    // cos
    func_lib.add(Func {
        id: "01897c54-ceb5-e603-ebde-c6904a8ef6e5".into(),
        name: "cos".to_string(),
        description: Some("Calculates the cosine of an angle in radians".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "a".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: Some((0.0).into()),
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "cos".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let cos = a.cos();

            outputs[0] = cos.into();
            Ok(())
        }),
        ..Default::default()
    });
    // tan
    func_lib.add(Func {
        id: "01897c55-1fda-2837-f4bd-75bea812a70e".into(),
        name: "tan".to_string(),
        description: Some("Calculates the tangent of an angle in radians".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "a".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: Some((0.0).into()),
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "tan".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let a: f64 = inputs[0].value.as_f64().unwrap();
            let tan = a.tan();

            outputs[0] = tan.into();
            Ok(())
        }),
        ..Default::default()
    });
    // asin
    func_lib.add(Func {
        id: "01897c55-6920-1641-593c-5a1d91c033cb".into(),
        name: "asin".to_string(),
        description: Some(
            "Calculates the arc sine (inverse sine), returns angle in radians".to_string(),
        ),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "sin".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: Some((0.0).into()),
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "asin".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let sin: f64 = inputs[0].value.as_f64().unwrap();
            let asin = sin.asin();

            outputs[0] = asin.into();
            Ok(())
        }),
        ..Default::default()
    });
    // acos
    func_lib.add(Func {
        id: "01897c55-a3ef-681e-6fbb-5133c96f720c".into(),
        name: "acos".to_string(),
        description: Some(
            "Calculates the arc cosine (inverse cosine), returns angle in radians".to_string(),
        ),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "cos".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: Some((1.0).into()),
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "acos".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let cos: f64 = inputs[0].value.as_f64().unwrap();
            let acos = cos.acos();

            outputs[0] = acos.into();
            Ok(())
        }),
        ..Default::default()
    });
    // atan
    func_lib.add(Func {
        id: "01897c55-e6f4-726c-5d4e-a2f90c4fc43b".into(),
        name: "atan".to_string(),
        description: Some(
            "Calculates the arc tangent (inverse tangent), returns angle in radians".to_string(),
        ),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![FuncInput {
            name: "tan".to_string(),
            required: true,
            data_type: DataType::Float,
            default_value: Some((0.0).into()),
            value_options: vec![],
        }],
        outputs: vec![FuncOutput {
            name: "atan".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);

            let tan: f64 = inputs[0].value.as_f64().unwrap();
            let atan = tan.atan();

            outputs[0] = atan.into();
            Ok(())
        }),
        ..Default::default()
    });
    // log
    func_lib.add(Func {
        id: "01897c56-8dde-c5f3-a389-f326fdf81b3a".into(),
        name: "log".to_string(),
        description: Some("Calculates the logarithm of a value with the given base".to_string()),
        behavior: FuncBehavior::Pure,
        terminal: false,
        category: "math".to_string(),
        inputs: vec![
            FuncInput {
                name: "value".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((1.0).into()),
                value_options: vec![],
            },
            FuncInput {
                name: "base".to_string(),
                required: true,
                data_type: DataType::Float,
                default_value: Some((10.0).into()),
                value_options: vec![],
            },
        ],
        outputs: vec![FuncOutput {
            name: "log".to_string(),
            data_type: DataType::Float,
        }],
        events: vec![],
        required_contexts: vec![],
        lambda: async_lambda!(move |_, _, _, inputs, _, outputs| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);

            let value: f64 = inputs[0].value.as_f64().unwrap();
            let base: f64 = inputs[1].value.as_f64().unwrap();
            let log = value.log(base);

            outputs[0] = log.into();
            Ok(())
        }),
        ..Default::default()
    });

    func_lib
}
