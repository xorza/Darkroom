use crate::DataType;
use crate::async_lambda;
use crate::graph::func::lambda::Invocation;
use crate::graph::func::{Func, FuncInput, FuncOutput};
use crate::library::Library;

/// The built-in system / utility nodes: logging and value-to-text conversion.
pub fn system_library() -> Library {
    let mut library = Library::default();

    // print: log the input value to the node log (info level), read
    // back by the editor. Sugar over `ContextManager::log`.
    library.add(
        Func::new("06b169d2-d3d8-43fb-ba80-c566fcffea44", "Print")
            .description("Logs any value to the node log.")
            .category("System")
            .sink()
            .input(
                FuncInput::required("Value", DataType::Any)
                    .description("Value of any type to write to the node's log (info level)."),
            )
            .lambda(async_lambda!(move |Invocation { ctx, inputs, .. }| {
                assert_eq!(inputs.len(), 1);
                ctx.info(inputs[0].to_value_string());
                Ok(())
            })),
    );

    library.add(
        Func::new("9731dfca-061b-421c-a193-4aa9a61d89b5", "To String")
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
            .lambda(async_lambda!(|Invocation {
                                       inputs, outputs, ..
                                   }| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(outputs.len(), 1);

                outputs[0] = inputs[0].to_value_string().into();
                Ok(())
            })),
    );

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
            .lambda(async_lambda!(|Invocation {
                                       inputs, outputs, ..
                                   }| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let mut result = inputs[0].to_value_string();
                result.push_str(&inputs[1].to_value_string());

                outputs[0] = result.into();
                Ok(())
            })),
    );

    library
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DynamicValue;
    use crate::testing::func_invoker::FuncInvoker;

    /// Invoke the `Concat` node's lambda with two runtime values and return its
    /// text output.
    async fn run_concat(a: DynamicValue, b: DynamicValue) -> String {
        let library = system_library();
        let concat = library.by_name("Concat").unwrap();
        let outputs = FuncInvoker::default()
            .call(concat, [a, b])
            .await
            .expect("Concat stringifies whatever it is handed");
        outputs[0].as_string().unwrap().to_owned()
    }

    #[test]
    fn concat_interface_is_two_any_inputs_to_string() {
        let library = system_library();
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
