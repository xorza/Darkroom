use super::*;

/// Nothing has run, so nothing is reported: an id absent from the program
/// resolves to no values at all, and a node that is present carries neither
/// delivered inputs nor produced outputs.
#[test]
fn before_execution_reports_no_values() {
    let e = TestEngine::over(TestGraph::sample());

    let nonexistent: NodeId = "00000000-0000-0000-0000-000000000000".into();
    assert!(e.engine.get_argument_values(&nonexistent).is_none());

    let inputs = e.inputs("sum");
    assert_eq!(inputs.len(), 2);
    assert!(inputs.iter().all(Option::is_none));
    assert!(e.outputs("sum").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn with_bound_outputs() {
    let mut e = TestEngine::over(TestGraph::sample_values(2, 5));

    e.run_sinks().await;

    // The two sources emit `Float` (their lambdas cast through `f64`), which
    // the `Int`-declared consumers read through the scalar coercion class —
    // so the variant is worth pinning, not just the number.
    assert!(matches!(
        e.inputs("sum")[0],
        Some(DynamicValue::Static(ConstValue::Float(v))) if v.approximately_eq(2.0)
    ));
    assert!(matches!(
        e.inputs("sum")[1],
        Some(DynamicValue::Static(ConstValue::Float(v))) if v.approximately_eq(5.0)
    ));
    assert_eq!(e.output_i64("sum", 0), Some(7), "2 + 5");

    assert_eq!(e.input_i64("mult", 0), Some(7));
    assert!(matches!(
        e.inputs("mult")[1],
        Some(DynamicValue::Static(ConstValue::Float(v))) if v.approximately_eq(5.0)
    ));
    assert_eq!(e.output_i64("mult", 0), Some(35), "7 * 5");

    assert_eq!(e.input_i64("Print", 0), Some(35));
    assert!(e.outputs("Print").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn with_none_binding() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| {
        g.edit_func("mult", |func| func.inputs[1].required = false);
        g.unbind("mult", 1);
    });

    e.run_sinks().await;

    let inputs = e.inputs("mult");
    assert_eq!(inputs.len(), 2);
    assert!(inputs[0].is_some());
    assert!(inputs[1].is_none(), "an unbound port delivers no value");
}
