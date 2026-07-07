use rand::{RngExt, SeedableRng};

use crate::async_lambda;
use crate::data::DataType;
use crate::library::Library;
use crate::node::function::{Func, FuncInput, FuncOutput};

/// The built-in math / string / print nodes.
pub fn basic_library() -> Library {
    let mut library = Library::default();

    // print: log the input string to the node log (info level), read
    // back by the editor. Sugar over `ContextManager::log`.
    library.add(
        Func::new("01896910-0790-AD1B-AA12-3F1437196789", "Print")
            .description("Logs any value to the node log.")
            .category("System")
            .terminal()
            .input(
                FuncInput::required("Value", DataType::Any)
                    .description("Value of any type to write to the node's log (info level)."),
            )
            .lambda(async_lambda!(move |ctx, _, _, inputs, _, _| {
                assert_eq!(inputs.len(), 1);
                ctx.info(inputs[0].value.to_value_string());
                Ok(())
            })),
    );

    // to string
    library.add(
        Func::new("01896a88-bf15-dead-4a15-5969da5a9e65", "To String")
            .description("Converts any value to its string representation.")
            .category("System")
            .pure()
            .input(
                FuncInput::required("Value", DataType::Any)
                    .description("Value of any type to convert to text."),
            )
            .output(
                FuncOutput::new("Text", DataType::String).description("The value's string form."),
            )
            .lambda(async_lambda!(|_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                outputs[0] = inputs[0].value.to_value_string().into();
                Ok(())
            })),
    );

    // concat
    library.add(
        Func::new("8854cccc-81d3-4e26-8b4f-e33d62e3117b", "Concat")
            .description(
                "Converts two values of any type to text and joins them (A followed by B).",
            )
            .category("System")
            .pure()
            .input(
                FuncInput::required("A", DataType::Any)
                    .description("First value; its text comes first."),
            )
            .input(
                FuncInput::required("B", DataType::Any)
                    .description("Second value; its text is appended after A."),
            )
            .output(
                FuncOutput::new("Text", DataType::String).description("A's text followed by B's."),
            )
            .lambda(async_lambda!(|_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let mut result = inputs[0].value.to_value_string();
                result.push_str(&inputs[1].value.to_value_string());

                outputs[0] = result.into();
                Ok(())
            })),
    );

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

                let min: f64 = inputs[0].value.as_f64().unwrap();
                let max: f64 = inputs[1].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
                let b: f64 = inputs[1].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
                let b: f64 = inputs[1].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
                let b: f64 = inputs[1].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
                let b: f64 = inputs[1].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
                let b: f64 = inputs[1].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
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

                let a: f64 = inputs[0].value.as_f64().unwrap();
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

                let sin: f64 = inputs[0].value.as_f64().unwrap();
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

                let cos: f64 = inputs[0].value.as_f64().unwrap();
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

                let tan: f64 = inputs[0].value.as_f64().unwrap();
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

                let value: f64 = inputs[0].value.as_f64().unwrap();
                let base: f64 = inputs[1].value.as_f64().unwrap();
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
    use crate::data::DynamicValue;
    use crate::node::func_lambda::{InvokeInput, OutputUsage};
    use crate::runtime::any_state::AnyState;
    use crate::runtime::context::ContextManager;
    use crate::runtime::shared_any_state::SharedAnyState;

    /// Invoke the `Concat` node's lambda with two runtime values and return its
    /// text output.
    async fn run_concat(a: DynamicValue, b: DynamicValue) -> String {
        let library = basic_library();
        let concat = library.by_name("Concat").unwrap();
        let mut ctx = ContextManager::default();
        let mut state = AnyState::default();
        let event_state = SharedAnyState::default();
        let inputs = [InvokeInput { value: a }, InvokeInput { value: b }];
        let mut outputs = [DynamicValue::Unbound];
        concat
            .lambda
            .invoke(
                &mut ctx,
                &mut state,
                &event_state,
                &inputs,
                &[OutputUsage::Needed(1)],
                &mut outputs,
            )
            .await
            .unwrap();
        outputs[0].as_string().unwrap().to_owned()
    }

    #[test]
    fn concat_interface_is_two_any_inputs_to_string() {
        let library = basic_library();
        let concat = library.by_name("Concat").unwrap();
        assert_eq!(concat.category, "System");
        assert_eq!(concat.inputs.len(), 2);
        assert_eq!(concat.inputs[0].data_type, DataType::Any);
        assert_eq!(concat.inputs[1].data_type, DataType::Any);
        assert_eq!(concat.outputs.len(), 1);
        assert_eq!(concat.outputs[0].ty.declared(), DataType::String);
    }

    #[tokio::test]
    async fn concat_stringifies_each_side_then_joins_a_before_b() {
        // Each side converts through `to_value_string` (unquoted, full-precision)
        // and the two texts join with no separator, A first.
        assert_eq!(run_concat(42i64.into(), " items".into()).await, "42 items");
        assert_eq!(run_concat(1.5f64.into(), true.into()).await, "1.5true");
        // Two strings join verbatim — no quotes, no gap.
        assert_eq!(run_concat("foo".into(), "bar".into()).await, "foobar");
        // Order matters: swapping operands swaps the halves.
        assert_eq!(run_concat("bar".into(), "foo".into()).await, "barfoo");
    }
}
