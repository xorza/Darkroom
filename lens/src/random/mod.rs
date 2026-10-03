use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use scenarium::FuncId;
use scenarium::Invocation;
use scenarium::async_lambda;
use scenarium::{DataType, Func, FuncInput, FuncOutput, Library};

const RANDOM_FUNC_ID: FuncId = FuncId::literal("01897928-66cd-52cb-abeb-a5bfd7f3763e");

fn scale_random(unit: f64, min: f64, max: f64) -> f64 {
    min + (max - min) * unit
}

fn random_func() -> Func {
    Func::new(
        RANDOM_FUNC_ID,
        "Random",
        async_lambda!(move |Invocation {
                                state: cache,
                                inputs,
                                outputs,
                                ..
                            }| {
            debug_assert_eq!(inputs.len(), 2);
            debug_assert_eq!(outputs.len(), 1);
            let rng = cache.get_or_insert_with(|| StdRng::from_rng(&mut rand::rng()));
            let min = inputs[0].required_f64();
            let max = inputs[1].required_f64();
            outputs[0] = scale_random(rng.random::<f64>(), min, max).into();
            Ok(())
        }),
    )
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
    .output(FuncOutput::new("Value", DataType::Float).description("A random number in [Min, Max)."))
}

pub fn random_library() -> Library {
    let mut library = Library::default();
    library.add(random_func());
    library
}

#[cfg(test)]
mod tests;
