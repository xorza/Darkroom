//! The Preview node: an ordinary library func whose lambda hands its input
//! straight to the GUI instead of producing an output.
//!
//! Declared here rather than in `gui/` because every frontend shares
//! [`RuntimeLibrary`](crate::core::runtime_library::RuntimeLibrary) — a document
//! holding a preview node must still compile headless, under the TUI, and for
//! the script host, or opening it would fail with a missing func.
//!
//! The value crosses threads through [`PreviewSink`], which the closure
//! captures at library-composition time. Scenarium knows nothing about any of
//! this: the node is a sink (so an ambient run reaches it), it reads its input
//! like any consumer, and `Func::entry_only` is the one thing the engine
//! enforces on its behalf.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use scenarium::{
    DataType, DynamicValue, ExecutionNodeId, Func, FuncId, FuncInput, Invocation, Library,
    async_lambda,
};

/// Stable `FuncId` for the preview node. Persisted in every document that holds
/// one, so it must never change.
const PREVIEW_FUNC_ID: &str = "7d08e8c7-fd22-46d4-bb86-c0bd3c9e76fe";

/// Where a preview node's value waits between the worker thread that produced
/// it and the frame that draws it.
///
/// A map rather than a queue, keyed by the node that published: a preview shows
/// *the* current value, so an older one for the same node is not just
/// redundant, it is memory held for nothing. Latest-wins keeps the sink bounded
/// at one value per preview node however fast runs arrive — which matters when
/// the values are full-resolution images.
#[derive(Debug, Default)]
pub(crate) struct PreviewSink {
    latest: Mutex<HashMap<ExecutionNodeId, DynamicValue>>,
}

impl PreviewSink {
    /// Worker side: publish `value` as `e_node_id`'s current one, dropping
    /// whatever it was showing before.
    fn publish(&self, e_node_id: ExecutionNodeId, value: DynamicValue) {
        self.latest.lock().unwrap().insert(e_node_id, value);
    }

    /// GUI side: take everything published since the last drain. Empty on an
    /// idle frame, which is the common case.
    pub(crate) fn drain(&self) -> Vec<(ExecutionNodeId, DynamicValue)> {
        let mut latest = self.latest.lock().unwrap();
        latest.drain().collect()
    }
}

/// The preview func, bound to the sink its lambda publishes into.
///
/// `sink()` so an ambient sinks run reaches it — that is what makes a preview
/// refresh without the editor naming it as a seed. `uncacheable()` because it
/// has no output to persist, and it stays `Impure` (the default) because
/// `Func::validate` refuses an outputless func that claims to be pure.
/// `entry_only()` because one node backs one on-screen card: inside a
/// definition instanced twice it would have two live occurrences behind one
/// identity, and the last to finish would win arbitrarily.
pub(crate) fn preview_func(sink: Arc<PreviewSink>) -> Func {
    Func::new(PREVIEW_FUNC_ID, "Preview")
        .category("System")
        .sink()
        .uncacheable()
        .entry_only()
        .description(
            "Shows the value wired into it. The value goes to the editor \
             rather than to a consumer, so watching one never changes what the \
             rest of the graph computes. Only allowed in the top-level graph.",
        )
        .input(
            FuncInput::optional("Value", DataType::Any)
                .description("The value to show. Anything can be wired here."),
        )
        .lambda(async_lambda!(
            move |Invocation { ctx, inputs, .. }| { sink = Arc::clone(&sink) } => {
                // `current_node` is the only thing in the invocation that says
                // *which* preview this is — the editor routes on it.
                sink.publish(ctx.current_node(), std::mem::take(&mut inputs[0]));
                Ok(())
            }
        ))
}

/// The preview func as the current library registered it, or `None` when the
/// document's library has lost it — the editor's "add a preview here" action
/// builds its node from this rather than re-declaring the interface.
pub(crate) fn registered(library: &Library) -> Option<&Func> {
    library.funcs().find(|func| is_preview(func.id))
}

/// Whether `func_id` is the preview func — what the scene projection asks to
/// decide a node draws a value card instead of the usual body.
pub(crate) fn is_preview(func_id: FuncId) -> bool {
    func_id == PREVIEW_FUNC_ID.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::{AnyState, ContextManager, OutputDemand, SharedAnyState, StaticValue};

    #[test]
    fn the_declaration_is_an_entry_only_sink_that_produces_nothing() {
        let func = preview_func(Arc::default());
        assert!(func.sink, "an ambient sinks run must reach it");
        assert!(func.entry_only, "one node backs one card");
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
        let first = ExecutionNodeId::default();

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
}
