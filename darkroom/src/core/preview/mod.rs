//! The Preview node: an ordinary library func whose lambda hands its input
//! straight to the GUI instead of producing an output.
//!
//! Declared here rather than in `gui/` because every frontend shares
//! [`RuntimeLibrary`](crate::core::runtime_library::RuntimeLibrary) — a document
//! holding a preview node must still compile, or opening it would fail with a
//! missing func.
//!
//! The value crosses threads through [`PreviewSink`], which the closure
//! captures at library-composition time. Scenarium knows nothing about any of
//! this: the node is a sink (so an ambient run reaches it), it reads its input
//! like any consumer, and nothing about it is special to the engine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use scenarium::{
    DataType, DynamicValue, Func, FuncId, FuncInput, Invocation, Library, NodeId, async_lambda,
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
    latest: Mutex<HashMap<NodeId, DynamicValue>>,
}

impl PreviewSink {
    /// Worker side: publish `value` as `node_id`'s current one, dropping
    /// whatever it was showing before.
    fn publish(&self, node_id: NodeId, value: DynamicValue) {
        self.latest.lock().unwrap().insert(node_id, value);
    }

    /// GUI side: take everything published since the last drain. Empty on an
    /// idle frame, which is the common case.
    pub(crate) fn drain(&self) -> Vec<(NodeId, DynamicValue)> {
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
pub(crate) fn preview_func(sink: Arc<PreviewSink>) -> Func {
    Func::new(PREVIEW_FUNC_ID, "Preview")
        .category("System")
        .sink()
        .uncacheable()
        .description(
            "Shows the value wired into it. The value goes to the editor \
             rather than to a consumer, so watching one never changes what the \
             rest of the graph computes.",
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
mod tests;
