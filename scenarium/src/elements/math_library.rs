use rand::{RngExt, SeedableRng};

use crate::async_lambda;
use crate::data::DataType;
use crate::library::Library;
use crate::node::func_lambda::{InvokeError, InvokeInput};
use crate::node::function::{Func, FuncInput, FuncOutput};

/// A lambda input coerced to `f64`, failing the invoke instead of panicking: a
/// non-numeric value can legally reach a Float port through an `Any`-typed
/// subgraph boundary, which compile-time validation can't see through — and a
/// lambda panic would take down the whole worker task.
fn float_input(inputs: &[InvokeInput], idx: usize) -> Result<f64, InvokeError> {
    inputs[idx].value.as_f64().ok_or_else(|| {
        InvokeError::External(anyhow::anyhow!(
            "input {} is not a number: {:?}",
            idx,
            inputs[idx].value
        ))
    })
}

/// The built-in math nodes.
pub fn math_library() -> Library {
    let mut library = Library::default();

    // random
    library.add(
        Func::new("01897928-66cd-52cb-abeb-a5bfd7f3763e", "Random")
            .description("Generates a random float between min and max values.")
            .category("Math")
            .input(
                FuncInput::required("Min", DataType::Float)
                    .description("Lower bound (inclusive).")
                    .default(0.0),
            )
            .input(
                FuncInput::required("Max", DataType::Float)
                    .description("Upper bound (exclusive).")
                    .default(1.0),
            )
            .output(
                FuncOutput::new("Value", DataType::Float)
                    .description("A random number in [Min, Max)."),
            )
            .lambda(async_lambda!(move |_, cache, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let rng =
                    cache.get_or_default_with(|| rand::rngs::StdRng::from_rng(&mut rand::rng()));

                let min: f64 = float_input(inputs, 0)?;
                let max: f64 = float_input(inputs, 1)?;
                let random = rng.random::<f64>();
                let result = min + (max - min) * random;

                outputs[0] = result.into();
                Ok(())
            })),
    );

    // add
    library.add(
        Func::new("01897c4c-ac6a-84c0-d0b7-17d49e1ae2ee", "Add")
            .description("Adds two float values (A + B).")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("A", DataType::Float)
                    .description("First addend.")
                    .default(0.0),
            )
            .input(
                FuncInput::required("B", DataType::Float)
                    .description("Second addend.")
                    .default(1.0),
            )
            .output(FuncOutput::new("Sum", DataType::Float).description("A + B."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let b: f64 = float_input(inputs, 1)?;
                let result = a + b;

                outputs[0] = result.into();
                Ok(())
            })),
    );

    // subtract
    library.add(
        Func::new("01897c50-229e-f5e4-1c60-7f1e14531da2", "Subtract")
            .description("Subtracts the second value from the first (A − B).")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("A", DataType::Float)
                    .description("Minuend.")
                    .default(0.0),
            )
            .input(
                FuncInput::required("B", DataType::Float)
                    .description("Subtrahend.")
                    .default(1.0),
            )
            .output(FuncOutput::new("Difference", DataType::Float).description("A − B."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let b: f64 = float_input(inputs, 1)?;
                let result = a - b;

                outputs[0] = result.into();
                Ok(())
            })),
    );

    // multiply
    library.add(
        Func::new("01897c50-d510-55bf-8cb9-545a62cc76cc", "Multiply")
            .description("Multiplies two float values (A × B).")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("A", DataType::Float)
                    .description("First factor.")
                    .default(0.0),
            )
            .input(
                FuncInput::required("B", DataType::Float)
                    .description("Second factor.")
                    .default(1.0),
            )
            .output(FuncOutput::new("Product", DataType::Float).description("A × B."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let b: f64 = float_input(inputs, 1)?;
                let result = a * b;

                outputs[0] = result.into();
                Ok(())
            })),
    );

    // divide
    library.add(
        Func::new("01897c50-2b4e-4f0e-8f0a-5b0b8b2b4b4b", "Divide")
            .description(
                "Divides the first value by the second, outputs both quotient and remainder.",
            )
            .category("Math")
            .pure()
            .input(
                FuncInput::required("A", DataType::Float)
                    .description("Dividend.")
                    .default(0.0),
            )
            .input(
                FuncInput::required("B", DataType::Float)
                    .description("Divisor.")
                    .default(1.0),
            )
            .output(FuncOutput::new("Quotient", DataType::Float).description("A ÷ B."))
            .output(FuncOutput::new("Remainder", DataType::Float).description("A mod B."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 2);

                let a: f64 = float_input(inputs, 0)?;
                let b: f64 = float_input(inputs, 1)?;
                let divide = a / b;
                let modulo = a % b;

                outputs[0] = divide.into();
                outputs[1] = modulo.into();
                Ok(())
            })),
    );

    // power
    library.add(
        Func::new("01897c52-ac50-733e-aeeb-7018fd84c264", "Power")
            .description("Raises the first value to the power of the second (Base^Exponent).")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Base", DataType::Float)
                    .description("The base.")
                    .default(0.0),
            )
            .input(
                FuncInput::required("Exponent", DataType::Float)
                    .description("The exponent.")
                    .default(1.0),
            )
            .output(
                FuncOutput::new("Result", DataType::Float).description("Base raised to Exponent."),
            )
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let b: f64 = float_input(inputs, 1)?;
                let power = a.powf(b);

                outputs[0] = power.into();
                Ok(())
            })),
    );

    // sqrt
    library.add(
        Func::new("01897c53-a3d7-e716-b80a-0ba98661413a", "Square Root")
            .description("Calculates the square root of a value.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Value", DataType::Float)
                    .description("Number to take the square root of.")
                    .default(0.0),
            )
            .output(FuncOutput::new("Root", DataType::Float).description("√Value."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let sqrt = a.sqrt();

                outputs[0] = sqrt.into();
                Ok(())
            })),
    );

    // sin
    library.add(
        Func::new("01897c54-8671-5d7c-db4c-aca72865a5a6", "Sine")
            .description("Calculates the sine of an angle in radians.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Angle", DataType::Float)
                    .description("Angle in radians.")
                    .default(0.0),
            )
            .output(FuncOutput::new("Sine", DataType::Float).description("sin(Angle)."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let sin = a.sin();

                outputs[0] = sin.into();
                Ok(())
            })),
    );

    // cos
    library.add(
        Func::new("01897c54-ceb5-e603-ebde-c6904a8ef6e5", "Cosine")
            .description("Calculates the cosine of an angle in radians.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Angle", DataType::Float)
                    .description("Angle in radians.")
                    .default(0.0),
            )
            .output(FuncOutput::new("Cosine", DataType::Float).description("cos(Angle)."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let cos = a.cos();

                outputs[0] = cos.into();
                Ok(())
            })),
    );

    // tan
    library.add(
        Func::new("01897c55-1fda-2837-f4bd-75bea812a70e", "Tangent")
            .description("Calculates the tangent of an angle in radians.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Angle", DataType::Float)
                    .description("Angle in radians.")
                    .default(0.0),
            )
            .output(FuncOutput::new("Tangent", DataType::Float).description("tan(Angle)."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let a: f64 = float_input(inputs, 0)?;
                let tan = a.tan();

                outputs[0] = tan.into();
                Ok(())
            })),
    );

    // asin
    library.add(
        Func::new("01897c55-6920-1641-593c-5a1d91c033cb", "Arcsine")
            .description("Calculates the arc sine (inverse sine), returns angle in radians.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Sine", DataType::Float)
                    .description("Sine value in [−1, 1].")
                    .default(0.0),
            )
            .output(FuncOutput::new("Angle", DataType::Float).description("Angle in radians."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let sin: f64 = float_input(inputs, 0)?;
                let asin = sin.asin();

                outputs[0] = asin.into();
                Ok(())
            })),
    );

    // acos
    library.add(
        Func::new("01897c55-a3ef-681e-6fbb-5133c96f720c", "Arccosine")
            .description("Calculates the arc cosine (inverse cosine), returns angle in radians.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Cosine", DataType::Float)
                    .description("Cosine value in [−1, 1].")
                    .default(1.0),
            )
            .output(FuncOutput::new("Angle", DataType::Float).description("Angle in radians."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let cos: f64 = float_input(inputs, 0)?;
                let acos = cos.acos();

                outputs[0] = acos.into();
                Ok(())
            })),
    );

    // atan
    library.add(
        Func::new("01897c55-e6f4-726c-5d4e-a2f90c4fc43b", "Arctangent")
            .description("Calculates the arc tangent (inverse tangent), returns angle in radians.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Tangent", DataType::Float)
                    .description("Tangent value.")
                    .default(0.0),
            )
            .output(FuncOutput::new("Angle", DataType::Float).description("Angle in radians."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let tan: f64 = float_input(inputs, 0)?;
                let atan = tan.atan();

                outputs[0] = atan.into();
                Ok(())
            })),
    );

    // log
    library.add(
        Func::new("01897c56-8dde-c5f3-a389-f326fdf81b3a", "Logarithm")
            .description("Calculates the logarithm of a value with the given base.")
            .category("Math")
            .pure()
            .input(
                FuncInput::required("Value", DataType::Float)
                    .description("Number to take the logarithm of.")
                    .default(1.0),
            )
            .input(
                FuncInput::required("Base", DataType::Float)
                    .description("Logarithm base.")
                    .default(10.0),
            )
            .output(FuncOutput::new("Result", DataType::Float).description("log_Base(Value)."))
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let value: f64 = float_input(inputs, 0)?;
                let base: f64 = float_input(inputs, 1)?;
                let log = value.log(base);

                outputs[0] = log.into();
                Ok(())
            })),
    );

    library
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{DynamicValue, StaticValue};
    use crate::node::func_lambda::OutputUsage;
    use crate::runtime::any_state::AnyState;
    use crate::runtime::context::ContextManager;
    use crate::runtime::shared_any_state::SharedAnyState;

    async fn invoke(
        name: &str,
        input_values: &[DynamicValue],
    ) -> Result<DynamicValue, InvokeError> {
        let lib = math_library();
        let func = lib.by_name(name).unwrap();
        let inputs: Vec<InvokeInput> = input_values
            .iter()
            .map(|v| InvokeInput { value: v.clone() })
            .collect();
        let usage = vec![OutputUsage::Needed(1); func.outputs.len()];
        let mut outputs = vec![DynamicValue::Unbound; func.outputs.len()];
        func.lambda
            .invoke(
                &mut ContextManager::default(),
                &mut AnyState::default(),
                &SharedAnyState::default(),
                &inputs,
                &usage,
                &mut outputs,
            )
            .await?;
        Ok(outputs[0].clone())
    }

    fn float(v: f64) -> DynamicValue {
        StaticValue::Float(v).into()
    }

    /// The binary ops compute their hand-checked results, and a non-numeric input —
    /// reachable at runtime through an `Any`-typed subgraph boundary — is an invoke
    /// *error*, never a panic (a panic would kill the whole worker task).
    #[tokio::test]
    async fn ops_compute_and_reject_non_numeric_inputs() {
        // a=2, b=3: 2+3=5, 2−3=−1, 2×3=6, 2³=8.
        for (name, expected) in [
            ("Add", 5.0),
            ("Subtract", -1.0),
            ("Multiply", 6.0),
            ("Power", 8.0),
        ] {
            let out = invoke(name, &[float(2.0), float(3.0)]).await.unwrap();
            assert_eq!(out.as_f64(), Some(expected), "{name}(2, 3)");
        }

        let text = DynamicValue::Static(StaticValue::String("not a number".into()));
        assert!(
            invoke("Add", &[text.clone(), float(3.0)]).await.is_err(),
            "a String left operand errors instead of panicking"
        );
        assert!(
            invoke("Add", &[float(2.0), text.clone()]).await.is_err(),
            "a String right operand errors instead of panicking"
        );
        assert!(
            invoke("Sine", &[text]).await.is_err(),
            "unary ops error the same way"
        );
    }
}
