use super::*;
use scenarium::{AnyState, ContextManager, OutputDemand, SharedAnyState, StaticValue};

#[test]
fn the_declaration_is_a_sink_that_produces_nothing() {
    let func = preview_func(Arc::default());
    assert!(func.sink, "an ambient sinks run must reach it");
    assert!(func.uncacheable, "no output to persist");
    assert!(func.outputs.is_empty());
    assert!(func.events.is_empty());
    assert_eq!(func.inputs.len(), 1);
    assert!(
        !func.inputs[0].required,
        "an unwired preview must not read as a missing input"
    );
    assert!(
        !func.inputs[0].const_only,
        "a preview exists to watch an upstream wire"
    );
    assert_eq!(func.inputs[0].data_type, DataType::Any);
    func.validate().unwrap();
    assert!(is_preview(func.id));
}

/// The lambda publishes under the node it was invoked as, and a second run
/// of the same node replaces rather than accumulates — the bound that keeps
/// a full-resolution image from piling up.
#[tokio::test]
async fn invoking_publishes_the_latest_value_per_node() {
    let sink = Arc::new(PreviewSink::default());
    let func = preview_func(Arc::clone(&sink));
    let first = NodeId::default();

    let invoke = async |value: i64| {
        let mut ctx = ContextManager::default();
        ctx.set_current_node(first);
        let mut inputs = [DynamicValue::Static(StaticValue::Int(value))];
        func.lambda
            .invoke(Invocation {
                ctx: &mut ctx,
                state: &mut AnyState::default(),
                event_state: &SharedAnyState::default(),
                inputs: &mut inputs,
                demand: &[] as &[OutputDemand],
                outputs: &mut [],
            })
            .await
            .unwrap();
        // The lambda takes the value, so the slot is left empty.
        assert!(matches!(inputs[0], DynamicValue::Unbound));
    };

    invoke(7).await;
    invoke(8).await;
    let drained = sink.drain();
    assert_eq!(drained.len(), 1, "one entry per node, not per invoke");
    assert_eq!(drained[0].0, first);
    assert_eq!(drained[0].1.as_i64(), Some(8), "the later value wins");
    assert!(sink.drain().is_empty(), "a drain empties the sink");
}

/// An invoke with no attribution is an executor bug, not a runtime state,
/// so it trips scenarium's own invariant rather than silently showing
/// nothing forever.
#[tokio::test]
#[should_panic(expected = "only readable inside a lambda invoke")]
async fn an_unattributed_invoke_is_a_bug_not_a_silent_no_op() {
    let func = preview_func(Arc::default());
    let mut inputs = [DynamicValue::Static(StaticValue::Int(1))];
    func.lambda
        .invoke(Invocation {
            ctx: &mut ContextManager::default(),
            state: &mut AnyState::default(),
            event_state: &SharedAnyState::default(),
            inputs: &mut inputs,
            demand: &[] as &[OutputDemand],
            outputs: &mut [],
        })
        .await
        .unwrap();
}
