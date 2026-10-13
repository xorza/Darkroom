use crate::DataType;
use crate::async_lambda;
use crate::graph::func::lambda::{FuncLambda, Invocation};
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
const DIVIDE_FUNC_ID: FuncId = FuncId::literal("65011378-e8e2-4471-960f-40c638089df4");

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

impl FloatInputSpec {
    fn declare(self) -> FuncInput {
        FuncInput::required(self.name, DataType::Float)
            .description(self.description)
            .default(self.default)
    }
}

impl FloatOutputSpec {
    fn declare(self) -> FuncOutput {
        FuncOutput::new(self.name, DataType::Float).description(self.description)
    }
}

/// A pure `Math` func over `N` float inputs and one float output: everything
/// but the operation, which [`unary`](FloatFunc::unary) and
/// [`binary`](FloatFunc::binary) take by arity.
#[derive(Debug, Clone, Copy)]
struct FloatFunc<const N: usize> {
    id: FuncId,
    name: &'static str,
    description: &'static str,
    inputs: [FloatInputSpec; N],
    output: FloatOutputSpec,
}

impl<const N: usize> FloatFunc<N> {
    fn declare(self, lambda: FuncLambda) -> Func {
        Func::new(self.id, self.name, lambda)
            .description(self.description)
            .category("Math")
            .pure()
            .inputs(self.inputs.map(FloatInputSpec::declare))
            .output(self.output.declare())
    }
}

impl FloatFunc<1> {
    fn unary(self, operation: fn(f64) -> f64) -> Func {
        self.declare(async_lambda!(move |Invocation {
                                             inputs, outputs, ..
                                         }| {
            assert_eq!(inputs.len(), 1);
            assert_eq!(outputs.len(), 1);
            outputs[0] = operation(inputs[0].required_f64()).into();
            Ok(())
        }))
    }
}

impl FloatFunc<2> {
    fn binary(self, operation: fn(f64, f64) -> f64) -> Func {
        self.declare(async_lambda!(move |Invocation {
                                             inputs, outputs, ..
                                         }| {
            assert_eq!(inputs.len(), 2);
            assert_eq!(outputs.len(), 1);
            outputs[0] = operation(inputs[0].required_f64(), inputs[1].required_f64()).into();
            Ok(())
        }))
    }
}

pub fn math_library() -> Library {
    let mut library = Library::default();

    library.add(
        FloatFunc {
            id: ADD_FUNC_ID,
            name: "Add",
            description: "Adds two float values (A + B).",
            inputs: [
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
            output: FloatOutputSpec {
                name: "Sum",
                description: "A + B.",
            },
        }
        .binary(|a, b| a + b),
    );
    library.add(
        FloatFunc {
            id: SUBTRACT_FUNC_ID,
            name: "Subtract",
            description: "Subtracts the second value from the first (A − B).",
            inputs: [
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
            output: FloatOutputSpec {
                name: "Difference",
                description: "A − B.",
            },
        }
        .binary(|a, b| a - b),
    );
    library.add(
        FloatFunc {
            id: MULTIPLY_FUNC_ID,
            name: "Multiply",
            description: "Multiplies two float values (A × B).",
            inputs: [
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
            output: FloatOutputSpec {
                name: "Product",
                description: "A × B.",
            },
        }
        .binary(|a, b| a * b),
    );
    library.add(divide_func());
    library.add(
        FloatFunc {
            id: POWER_FUNC_ID,
            name: "Power",
            description: "Raises the first value to the power of the second (Base^Exponent).",
            inputs: [
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
            output: FloatOutputSpec {
                name: "Result",
                description: "Base raised to Exponent.",
            },
        }
        .binary(f64::powf),
    );
    library.add(
        FloatFunc {
            id: SQUARE_ROOT_FUNC_ID,
            name: "Square Root",
            description: "Calculates the square root of a value.",
            inputs: [FloatInputSpec {
                name: "Value",
                description: "Number to take the square root of.",
                default: 0.0,
            }],
            output: FloatOutputSpec {
                name: "Root",
                description: "√Value.",
            },
        }
        .unary(f64::sqrt),
    );

    for function in trigonometry_funcs() {
        library.add(function);
    }

    library.add(
        FloatFunc {
            id: LOGARITHM_FUNC_ID,
            name: "Logarithm",
            description: "Calculates the logarithm of a value with the given base.",
            inputs: [
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
            output: FloatOutputSpec {
                name: "Result",
                description: "log_Base(Value).",
            },
        }
        .binary(f64::log),
    );

    library
}

fn trigonometry_funcs() -> [Func; 6] {
    [
        FloatFunc {
            id: SINE_FUNC_ID,
            name: "Sine",
            description: "Calculates the sine of an angle in radians.",
            inputs: [FloatInputSpec {
                name: "Angle",
                description: "Angle in radians.",
                default: 0.0,
            }],
            output: FloatOutputSpec {
                name: "Sine",
                description: "sin(Angle).",
            },
        }
        .unary(f64::sin),
        FloatFunc {
            id: COSINE_FUNC_ID,
            name: "Cosine",
            description: "Calculates the cosine of an angle in radians.",
            inputs: [FloatInputSpec {
                name: "Angle",
                description: "Angle in radians.",
                default: 0.0,
            }],
            output: FloatOutputSpec {
                name: "Cosine",
                description: "cos(Angle).",
            },
        }
        .unary(f64::cos),
        FloatFunc {
            id: TANGENT_FUNC_ID,
            name: "Tangent",
            description: "Calculates the tangent of an angle in radians.",
            inputs: [FloatInputSpec {
                name: "Angle",
                description: "Angle in radians.",
                default: 0.0,
            }],
            output: FloatOutputSpec {
                name: "Tangent",
                description: "tan(Angle).",
            },
        }
        .unary(f64::tan),
        FloatFunc {
            id: ARCSINE_FUNC_ID,
            name: "Arcsine",
            description: "Calculates the arc sine (inverse sine), returns angle in radians.",
            inputs: [FloatInputSpec {
                name: "Sine",
                description: "Sine value in [−1, 1].",
                default: 0.0,
            }],
            output: FloatOutputSpec {
                name: "Angle",
                description: "Angle in radians.",
            },
        }
        .unary(f64::asin),
        FloatFunc {
            id: ARCCOSINE_FUNC_ID,
            name: "Arccosine",
            description: "Calculates the arc cosine (inverse cosine), returns angle in radians.",
            inputs: [FloatInputSpec {
                name: "Cosine",
                description: "Cosine value in [−1, 1].",
                default: 1.0,
            }],
            output: FloatOutputSpec {
                name: "Angle",
                description: "Angle in radians.",
            },
        }
        .unary(f64::acos),
        FloatFunc {
            id: ARCTANGENT_FUNC_ID,
            name: "Arctangent",
            description: "Calculates the arc tangent (inverse tangent), returns angle in radians.",
            inputs: [FloatInputSpec {
                name: "Tangent",
                description: "Tangent value.",
                default: 0.0,
            }],
            output: FloatOutputSpec {
                name: "Angle",
                description: "Angle in radians.",
            },
        }
        .unary(f64::atan),
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
            let dividend = inputs[0].required_f64();
            let divisor = inputs[1].required_f64();
            outputs[0] = (dividend / divisor).into();
            outputs[1] = (dividend % divisor).into();
            Ok(())
        }),
    )
    .description("Divides the first value by the second, outputs both quotient and remainder.")
    .category("Math")
    .pure()
    .input(
        FloatInputSpec {
            name: "A",
            description: "Dividend.",
            default: 0.0,
        }
        .declare(),
    )
    .input(
        FloatInputSpec {
            name: "B",
            description: "Divisor.",
            default: 1.0,
        }
        .declare(),
    )
    .output(
        FloatOutputSpec {
            name: "Quotient",
            description: "A ÷ B.",
        }
        .declare(),
    )
    .output(
        FloatOutputSpec {
            name: "Remainder",
            description: "A mod B.",
        }
        .declare(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::func::error::InvokeError;
    use crate::internals::func_invoker::FuncInvoker;
    use crate::{ConstValue, DynamicValue};

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
    async fn operations_compute_exact_results() {
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
    }
}
