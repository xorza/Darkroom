use scenarium::{
    AnyState, ContextManager, DynamicValue, InvokeInput, OutputDemand, SharedAnyState,
};

use crate::utility::random::{random_library, scale_random};

#[test]
fn registers_random_func_and_scales_unit_values() {
    let library = random_library();
    let function = library.by_name("Random").unwrap();
    assert_eq!(function.inputs.len(), 2);
    assert_eq!(function.inputs[0].name, "Min");
    assert_eq!(function.inputs[1].name, "Max");
    assert_eq!(function.outputs.len(), 1);
    assert_eq!(function.outputs[0].name, "Value");

    let lower_quarter = scale_random(0.25, 2.0, 6.0);
    let shifted = scale_random(0.25, 6.0, 10.0);
    let upper_quarter = scale_random(0.75, 2.0, 6.0);
    assert_eq!(lower_quarter, 3.0);
    assert_eq!(shifted, 7.0);
    assert_eq!(upper_quarter, 5.0);
    assert_ne!(lower_quarter, shifted);
    assert_ne!(lower_quarter, upper_quarter);
}

#[tokio::test]
async fn equal_bounds_produce_that_exact_value() {
    let library = random_library();
    let function = library.by_name("Random").unwrap();
    let mut inputs = [
        InvokeInput {
            value: DynamicValue::from(4.25),
        },
        InvokeInput {
            value: DynamicValue::from(4.25),
        },
    ];
    let mut outputs = [DynamicValue::Unbound];
    function
        .lambda
        .invoke(
            &mut ContextManager::default(),
            &mut AnyState::default(),
            &SharedAnyState::default(),
            &mut inputs,
            &[OutputDemand::Produce],
            &mut outputs,
        )
        .await
        .unwrap();
    assert_eq!(outputs[0].as_f64(), Some(4.25));
}
