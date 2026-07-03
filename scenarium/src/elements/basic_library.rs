use rand::{RngExt, SeedableRng};
use strum::IntoEnumIterator;
use strum_macros::{Display, EnumIter};

use crate::async_lambda;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::library::Library;
use crate::node::func_lambda::InvokeInput;
use crate::node::function::{Func, FuncInput, ValueVariant};

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
pub fn basic_library() -> Library {
    let mut library = Library::default();

    // print: log the input string to the node log (info level), read
    // back by the editor. Sugar over `ContextManager::log`.
    library.add(
        Func::new("01896910-0790-AD1B-AA12-3F1437196789", "print")
            .description("Logs a string value to the node log")
            .category("math")
            .terminal()
            .input(FuncInput::required("value", DataType::String))
            .lambda(async_lambda!(move |ctx, _, _, inputs, _, _| {
                assert_eq!(inputs.len(), 1);
                let value: &str = inputs[0].value.as_string().unwrap();
                ctx.info(value);
                Ok(())
            })),
    );

    // math two argument operation
    library.add(
        Func::new("01896910-4BC9-77AA-6973-64CC1C56B9CE", "2 arg math")
            .description(
                "Performs a two-argument math operation (add, subtract, multiply, divide, modulo, \
                 power, log)",
            )
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float))
            .input(FuncInput::required("b", DataType::Float))
            .input(
                FuncInput::required("op", DataType::Int)
                    .default(Math2ArgOp::Add)
                    .variants(Math2ArgOp::list_variants()),
            )
            .output("result", DataType::Float)
            .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 3);
                assert_eq!(outputs.len(), 1);

                let op: Math2ArgOp = inputs[2].value.as_i64().unwrap().into();

                op.invoke(&inputs[0..2])
                    .map(|result| outputs[0] = result)
                    .expect("failed to invoke math two argument operation");
                Ok(())
            })),
    );

    // to string
    library.add(
        Func::new("01896a88-bf15-dead-4a15-5969da5a9e65", "float to string")
            .description("Converts a float value to its string representation")
            .category("math")
            .pure()
            .input(FuncInput::required("value", DataType::Float))
            .output("result", DataType::String)
            .lambda(async_lambda!(|_, _, _, inputs, _, outputs| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                let value: f64 = inputs[0].value.as_f64().unwrap();
                let result = value.to_string();

                outputs[0] = result.into();
                Ok(())
            })),
    );

    // random
    library.add(
        Func::new("01897928-66cd-52cb-abeb-a5bfd7f3763e", "random")
            .description("Generates a random float between min and max values")
            .category("math")
            .input(FuncInput::required("min", DataType::Float).default(0.0))
            .input(FuncInput::required("max", DataType::Float).default(1.0))
            .output("result", DataType::Float)
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
        Func::new("01897c4c-ac6a-84c0-d0b7-17d49e1ae2ee", "add")
            .description("Adds two float values (a + b)")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .input(FuncInput::required("b", DataType::Float).default(1.0))
            .output("result", DataType::Float)
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
        Func::new("01897c50-229e-f5e4-1c60-7f1e14531da2", "subtract")
            .description("Subtracts the second value from the first (a - b)")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .input(FuncInput::required("b", DataType::Float).default(1.0))
            .output("result", DataType::Float)
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
        Func::new("01897c50-d510-55bf-8cb9-545a62cc76cc", "multiply")
            .description("Multiplies two float values (a * b)")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .input(FuncInput::required("b", DataType::Float).default(1.0))
            .output("result", DataType::Float)
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
        Func::new("01897c50-2b4e-4f0e-8f0a-5b0b8b2b4b4b", "divide")
            .description("Divides the first value by the second, outputs both quotient and modulo")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .input(FuncInput::required("b", DataType::Float).default(1.0))
            .output("divide", DataType::Float)
            .output("modulo", DataType::Float)
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
        Func::new("01897c52-ac50-733e-aeeb-7018fd84c264", "power")
            .description("Raises the first value to the power of the second (a^b)")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .input(FuncInput::required("b", DataType::Float).default(1.0))
            .output("power", DataType::Float)
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
        Func::new("01897c53-a3d7-e716-b80a-0ba98661413a", "sqrt")
            .description("Calculates the square root of a value")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .output("sqrt", DataType::Float)
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
        Func::new("01897c54-8671-5d7c-db4c-aca72865a5a6", "sin")
            .description("Calculates the sine of an angle in radians")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .output("sin", DataType::Float)
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
        Func::new("01897c54-ceb5-e603-ebde-c6904a8ef6e5", "cos")
            .description("Calculates the cosine of an angle in radians")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .output("cos", DataType::Float)
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
        Func::new("01897c55-1fda-2837-f4bd-75bea812a70e", "tan")
            .description("Calculates the tangent of an angle in radians")
            .category("math")
            .pure()
            .input(FuncInput::required("a", DataType::Float).default(0.0))
            .output("tan", DataType::Float)
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
        Func::new("01897c55-6920-1641-593c-5a1d91c033cb", "asin")
            .description("Calculates the arc sine (inverse sine), returns angle in radians")
            .category("math")
            .pure()
            .input(FuncInput::required("sin", DataType::Float).default(0.0))
            .output("asin", DataType::Float)
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
        Func::new("01897c55-a3ef-681e-6fbb-5133c96f720c", "acos")
            .description("Calculates the arc cosine (inverse cosine), returns angle in radians")
            .category("math")
            .pure()
            .input(FuncInput::required("cos", DataType::Float).default(1.0))
            .output("acos", DataType::Float)
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
        Func::new("01897c55-e6f4-726c-5d4e-a2f90c4fc43b", "atan")
            .description("Calculates the arc tangent (inverse tangent), returns angle in radians")
            .category("math")
            .pure()
            .input(FuncInput::required("tan", DataType::Float).default(0.0))
            .output("atan", DataType::Float)
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
        Func::new("01897c56-8dde-c5f3-a389-f326fdf81b3a", "log")
            .description("Calculates the logarithm of a value with the given base")
            .category("math")
            .pure()
            .input(FuncInput::required("value", DataType::Float).default(1.0))
            .input(FuncInput::required("base", DataType::Float).default(10.0))
            .output("log", DataType::Float)
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
