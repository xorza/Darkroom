use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use scenarium::{
    DataType, Func, FuncInput, FuncLambda, FuncOutput, InvokeError, InvokeInput, Library,
};

fn float_input(inputs: &[InvokeInput], index: usize) -> Result<f64, InvokeError> {
    inputs[index].value.as_f64().ok_or_else(|| {
        InvokeError::External(anyhow::anyhow!(
            "input {} is not a number: {:?}",
            index,
            inputs[index].value
        ))
    })
}

fn scale_random(unit: f64, min: f64, max: f64) -> f64 {
    min + (max - min) * unit
}

fn random_func() -> Func {
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
            FuncOutput::new("Value", DataType::Float).description("A random number in [Min, Max)."),
        )
        .lambda(FuncLambda::new(move |_, cache, _, inputs, _, outputs| {
            Box::pin(async move {
                debug_assert_eq!(inputs.len(), 2);
                debug_assert_eq!(outputs.len(), 1);
                let rng = cache.get_or_default_with(|| StdRng::from_rng(&mut rand::rng()));
                let min = float_input(inputs, 0)?;
                let max = float_input(inputs, 1)?;
                outputs[0] = scale_random(rng.random::<f64>(), min, max).into();
                Ok(())
            })
        }))
}

pub fn random_library() -> Library {
    let mut library = Library::default();
    library.add(random_func());
    library
}

#[cfg(test)]
mod tests;
