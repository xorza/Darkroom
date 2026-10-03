use crate::DataType;
use crate::DynamicValue;
use crate::async_lambda;
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::Invocation;
use crate::graph::func::{Func, FuncInput, FuncOutput};
use crate::graph::identity::FuncId;
use crate::library::Library;

const ADD_FUNC_ID: FuncId = FuncId::literal("01897c4c-ac6a-84c0-d0b7-17d49e1ae2ee");
const SUBTRACT_FUNC_ID: FuncId = FuncId::literal("01897c50-229e-f5e4-1c60-7f1e14531da2");
const MULTIPLY_FUNC_ID: FuncId = FuncId::literal("01897c50-d510-55bf-8cb9-545a62cc76cc");
const POWER_FUNC_ID: FuncId = FuncId::literal("01897c52-ac50-733e-aeeb-7018fd84c264");
const SQUARE_ROOT_FUNC_ID: FuncId = FuncId::literal("01897c53-a3d7-e716-b80a-0ba98661413a");
const LOGARITHM_FUNC_ID: FuncId = FuncId::literal("01897c56-8dde-c5f3-a389-f326fdf81b3a");
const SINE_FUNC_ID: FuncId = FuncId::literal("01897c54-8671-5d7c-db4c-aca72865a5a6");
const COSINE_FUNC_ID: FuncId = FuncId::literal("01897c54-ceb5-e603-ebde-c6904a8ef6e5");
const TANGENT_FUNC_ID: FuncId = FuncId::literal("01897c55-1fda-2837-f4bd-75bea812a70e");
const ARCSINE_FUNC_ID: FuncId = FuncId::literal("01897c55-6920-1641-593c-5a1d91c033cb");
const ARCCOSINE_FUNC_ID: FuncId = FuncId::literal("01897c55-a3ef-681e-6fbb-5133c96f720c");
const ARCTANGENT_FUNC_ID: FuncId = FuncId::literal("01897c55-e6f4-726c-5d4e-a2f90c4fc43b");
const DIVIDE_FUNC_ID: FuncId = FuncId::literal("01897c50-2b4e-4f0e-8f0a-5b0b8b2b4b4b");

#[derive(Debug, Clone, Copy)]
struct FloatInputSpec {
    name: &'static str,
    description: &'static str,
    default: f64,
}

#[derive(Debug, Clone, Copy)]
struct FloatOutputSpec {
    name: &'static str,
    description: &'static str,
}

fn float_input(inputs: &[DynamicValue], idx: usize) -> Result<f64, InvokeError> {
    inputs[idx]
        .as_f64()
        .ok_or_else(|| InvokeError::invalid_input(idx, "a number", &inputs[idx]))
}

fn declared_input(spec: FloatInputSpec) -> FuncInput {
    FuncInput::required(spec.name, DataType::Float)
        .description(spec.description)
        .default(spec.default)
}

fn declared_output(spec: FloatOutputSpec) -> FuncOutput {
    FuncOutput::new(spec.name, DataType::Float).description(spec.description)
}

fn unary_float_func(
    id: FuncId,
    name: &'static str,
    description: &'static str,
    input: FloatInputSpec,
    output: FloatOutputSpec,
    operation: fn(f64) -> f64,
) -> Func {
    Func::new(
        id,
        name,
        async_lambda!(move |Invocation {
                                inputs, outputs, ..
                            }| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);
            outputs[0] = operation(float_input(inputs, 0)?).into();
            Ok(())
        }),
    )
    .description(description)
    .category("Math")
    .pure()
    .input(declared_input(input))
    .output(declared_output(output))
}

fn binary_float_func(
    id: FuncId,
    name: &'static str,
    description: &'static str,
    inputs: [FloatInputSpec; 2],
    output: FloatOutputSpec,
    operation: fn(f64, f64) -> f64,
) -> Func {
    Func::new(
        id,
        name,
        async_lambda!(move |Invocation {
                                inputs, outputs, ..
                            }| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);
            outputs[0] = operation(float_input(inputs, 0)?, float_input(inputs, 1)?).into();
            Ok(())
        }),
    )
    .description(description)
    .category("Math")
    .pure()
    .input(declared_input(inputs[0]))
    .input(declared_input(inputs[1]))
    .output(declared_output(output))
}

pub fn math_library() -> Library {
    let mut library = Library::default();

    library.add(binary_float_func(
        ADD_FUNC_ID,
        "Add",
        "Adds two float values (A + B).",
        [
            FloatInputSpec {
                name: "A",
                description: "First addend.",
                default: 0.0,
            },
            FloatInputSpec {
                name: "B",
                description: "Second addend.",
                default: 1.0,
            },
        ],
        FloatOutputSpec {
            name: "Sum",
            description: "A + B.",
        },
        |a, b| a + b,
    ));
    library.add(binary_float_func(
        SUBTRACT_FUNC_ID,
        "Subtract",
        "Subtracts the second value from the first (A − B).",
        [
            FloatInputSpec {
                name: "A",
                description: "Minuend.",
                default: 0.0,
            },
            FloatInputSpec {
                name: "B",
                description: "Subtrahend.",
                default: 1.0,
            },
        ],
        FloatOutputSpec {
            name: "Difference",
            description: "A − B.",
        },
        |a, b| a - b,
    ));
    library.add(binary_float_func(
        MULTIPLY_FUNC_ID,
        "Multiply",
        "Multiplies two float values (A × B).",
        [
            FloatInputSpec {
                name: "A",
                description: "First factor.",
                default: 0.0,
            },
            FloatInputSpec {
                name: "B",
                description: "Second factor.",
                default: 1.0,
            },
        ],
        FloatOutputSpec {
            name: "Product",
            description: "A × B.",
        },
        |a, b| a * b,
    ));
    library.add(divide_func());
    library.add(binary_float_func(
        POWER_FUNC_ID,
        "Power",
        "Raises the first value to the power of the second (Base^Exponent).",
        [
            FloatInputSpec {
                name: "Base",
                description: "The base.",
                default: 0.0,
            },
            FloatInputSpec {
                name: "Exponent",
                description: "The exponent.",
                default: 1.0,
            },
        ],
        FloatOutputSpec {
            name: "Result",
            description: "Base raised to Exponent.",
        },
        f64::powf,
    ));
    library.add(unary_float_func(
        SQUARE_ROOT_FUNC_ID,
        "Square Root",
        "Calculates the square root of a value.",
        FloatInputSpec {
            name: "Value",
            description: "Number to take the square root of.",
            default: 0.0,
        },
        FloatOutputSpec {
            name: "Root",
            description: "√Value.",
        },
        f64::sqrt,
    ));

    for function in trigonometry_funcs() {
        library.add(function);
    }

    library.add(binary_float_func(
        LOGARITHM_FUNC_ID,
        "Logarithm",
        "Calculates the logarithm of a value with the given base.",
        [
            FloatInputSpec {
                name: "Value",
                description: "Number to take the logarithm of.",
                default: 1.0,
            },
            FloatInputSpec {
                name: "Base",
                description: "Logarithm base.",
                default: 10.0,
            },
        ],
        FloatOutputSpec {
            name: "Result",
            description: "log_Base(Value).",
        },
        f64::log,
    ));

    library
}

fn trigonometry_funcs() -> [Func; 6] {
    [
        unary_float_func(
            SINE_FUNC_ID,
            "Sine",
            "Calculates the sine of an angle in radians.",
            FloatInputSpec {
                name: "Angle",
                description: "Angle in radians.",
                default: 0.0,
            },
            FloatOutputSpec {
                name: "Sine",
                description: "sin(Angle).",
            },
            f64::sin,
        ),
        unary_float_func(
            COSINE_FUNC_ID,
            "Cosine",
            "Calculates the cosine of an angle in radians.",
            FloatInputSpec {
                name: "Angle",
                description: "Angle in radians.",
                default: 0.0,
            },
            FloatOutputSpec {
                name: "Cosine",
                description: "cos(Angle).",
            },
            f64::cos,
        ),
        unary_float_func(
            TANGENT_FUNC_ID,
            "Tangent",
            "Calculates the tangent of an angle in radians.",
            FloatInputSpec {
                name: "Angle",
                description: "Angle in radians.",
                default: 0.0,
            },
            FloatOutputSpec {
                name: "Tangent",
                description: "tan(Angle).",
            },
            f64::tan,
        ),
        unary_float_func(
            ARCSINE_FUNC_ID,
            "Arcsine",
            "Calculates the arc sine (inverse sine), returns angle in radians.",
            FloatInputSpec {
                name: "Sine",
                description: "Sine value in [−1, 1].",
                default: 0.0,
            },
            FloatOutputSpec {
                name: "Angle",
                description: "Angle in radians.",
            },
            f64::asin,
        ),
        unary_float_func(
            ARCCOSINE_FUNC_ID,
            "Arccosine",
            "Calculates the arc cosine (inverse cosine), returns angle in radians.",
            FloatInputSpec {
                name: "Cosine",
                description: "Cosine value in [−1, 1].",
                default: 1.0,
            },
            FloatOutputSpec {
                name: "Angle",
                description: "Angle in radians.",
            },
            f64::acos,
        ),
        unary_float_func(
            ARCTANGENT_FUNC_ID,
            "Arctangent",
            "Calculates the arc tangent (inverse tangent), returns angle in radians.",
            FloatInputSpec {
                name: "Tangent",
                description: "Tangent value.",
                default: 0.0,
            },
            FloatOutputSpec {
                name: "Angle",
                description: "Angle in radians.",
            },
            f64::atan,
        ),
    ]
}

fn divide_func() -> Func {
    Func::new(
        DIVIDE_FUNC_ID,
        "Divide",
        async_lambda!(move |Invocation {
                                inputs, outputs, ..
                            }| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 2);
            let dividend = float_input(inputs, 0)?;
            let divisor = float_input(inputs, 1)?;
            outputs[0] = (dividend / divisor).into();
            outputs[1] = (dividend % divisor).into();
            Ok(())
        }),
    )
    .description("Divides the first value by the second, outputs both quotient and remainder.")
    .category("Math")
    .pure()
    .input(declared_input(FloatInputSpec {
        name: "A",
        description: "Dividend.",
        default: 0.0,
    }))
    .input(declared_input(FloatInputSpec {
        name: "B",
        description: "Divisor.",
        default: 1.0,
    }))
    .output(declared_output(FloatOutputSpec {
        name: "Quotient",
        description: "A ÷ B.",
    }))
    .output(declared_output(FloatOutputSpec {
        name: "Remainder",
        description: "A mod B.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConstValue;
    use crate::testing::func_invoker::FuncInvoker;

    async fn invoke(name: &str, values: &[DynamicValue]) -> Result<Vec<DynamicValue>, InvokeError> {
        let library = math_library();
        let func = library.by_name(name).unwrap();
        FuncInvoker::default().call(func, values.to_vec()).await
    }

    type Kernel = fn(f64) -> f64;

    fn float(value: f64) -> DynamicValue {
        ConstValue::Float(value).into()
    }

    #[tokio::test]
    async fn operations_compute_exact_results_and_reject_text() {
        for (name, expected) in [
            ("Add", 5.0),
            ("Subtract", -1.0),
            ("Multiply", 6.0),
            ("Power", 8.0),
            ("Logarithm", 0.630_929_753_571_457_4),
        ] {
            let outputs = invoke(name, &[float(2.0), float(3.0)]).await.unwrap();
            assert_eq!(outputs[0].as_f64(), Some(expected), "{name}(2, 3)");
        }

        let divide = invoke("Divide", &[float(7.0), float(3.0)]).await.unwrap();
        assert_eq!(divide[0].as_f64(), Some(7.0 / 3.0));
        assert_eq!(divide[1].as_f64(), Some(1.0));

        // One input at which the seven functions all differ, so a func wired
        // to another's kernel fails its row.
        let unary: [(&str, Kernel); 7] = [
            ("Square Root", f64::sqrt),
            ("Sine", f64::sin),
            ("Cosine", f64::cos),
            ("Tangent", f64::tan),
            ("Arcsine", f64::asin),
            ("Arccosine", f64::acos),
            ("Arctangent", f64::atan),
        ];
        for (name, kernel) in unary {
            let outputs = invoke(name, &[float(0.5)]).await.unwrap();
            assert_eq!(outputs[0].as_f64(), Some(kernel(0.5)), "{name}(0.5)");
        }
        let at_half: Vec<f64> = unary.iter().map(|(_, kernel)| kernel(0.5)).collect();
        for (i, value) in at_half.iter().enumerate() {
            assert!(
                !at_half[i + 1..].contains(value),
                "the fixture tells the ops apart"
            );
        }

        let text = DynamicValue::Static(ConstValue::String("not a number".into()));
        assert!(invoke("Add", &[text.clone(), float(3.0)]).await.is_err());
        assert!(invoke("Add", &[float(2.0), text.clone()]).await.is_err());
        assert!(invoke("Sine", &[text]).await.is_err());
    }
}
