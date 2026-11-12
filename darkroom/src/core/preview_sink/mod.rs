//! The Preview node: an ordinary library func whose lambda hands its input
//! straight to the GUI instead of producing an output.
//!
//! Declared here rather than in `gui/` because every frontend shares
//! [`RuntimeLibrary`](crate::core::runtime_library::RuntimeLibrary): a document holding a
//! preview node must still compile, or opening it would fail with a missing func.
//!
//! The value crosses threads through [`PreviewSink`], which the closure captures at
//! library-composition time. Scenarium knows nothing of this: the node is a sink, so an ambient
//! run reaches it, and it reads its input like any consumer.

use common::MutexExt;
use std::collections::HashMap;
use std::mem;
use std::sync::{Arc, Mutex};

use scenarium::{
    DataType, DynamicValue, Func, FuncId, FuncInput, Invocation, Library, NodeId, async_lambda,
};

/// Stable `FuncId` for the preview node. Persisted in every document that holds
/// one, so it must never change.
const PREVIEW_FUNC_ID: FuncId = FuncId::literal("7d08e8c7-fd22-46d4-bb86-c0bd3c9e76fe");

/// Where a preview node's value waits between the worker thread that produced
/// it and the frame that draws it.
///
/// A map keyed by the node that published, not a queue: a preview shows *the* current value, so
/// latest-wins keeps the sink bounded at one value per preview node however fast runs arrive,
/// which matters for full-resolution images.
#[derive(Debug, Default)]
pub(crate) struct PreviewSink {
    latest: Mutex<HashMap<NodeId, DynamicValue>>,
}

impl PreviewSink {
    /// Worker side: publish `value` as `node_id`'s current one, dropping the previous.
    fn publish(&self, node_id: NodeId, value: DynamicValue) {
        self.latest.lock_unpoisoned().insert(node_id, value);
    }

    /// GUI side: swap everything published since the last drain into the empty `buffer`. The
    /// maps trade places, so each keeps its capacity and a drain allocates nothing.
    pub(crate) fn drain_into(&self, buffer: &mut HashMap<NodeId, DynamicValue>) {
        debug_assert!(buffer.is_empty(), "the last drain's values were not taken");
        mem::swap(&mut *self.latest.lock_unpoisoned(), buffer);
    }
}

/// The preview func, bound to the sink its lambda publishes into.
///
/// `sink()` so an ambient sinks run reaches it, refreshing a preview without the editor naming
/// it as a seed. It stays `Impure` (the default) because `FuncBuilder::build` refuses an
/// outputless func that claims to be pure.
pub(crate) fn func(sink: Arc<PreviewSink>) -> Func {
    Func::builder(
        PREVIEW_FUNC_ID,
        "Preview",
        async_lambda!(
            move |Invocation { ctx, inputs, .. }| { sink = Arc::clone(&sink) } => {
                // The only thing in the invocation that says *which* preview this is.
                sink.publish(ctx.cur_node_id(), mem::take(&mut inputs[0]));
                Ok(())
            }
        ),
    )
    .category("System")
    .sink()
    .description(
        "Shows the value wired into it. The value goes to the editor \
         rather than to a consumer, so watching one never changes what the \
         rest of the graph computes.",
    )
    .input(
        FuncInput::optional("Value", DataType::Any)
            .description("The value to show. Anything can be wired here."),
    )
    .build()
}

/// The preview func as the current library registered it, or `None` when the library has lost
/// it; the editor's "add a preview here" action builds its node from this.
pub(crate) fn registered(library: &Library) -> Option<&Func> {
    library.by_id(PREVIEW_FUNC_ID)
}

/// Whether `func_id` is the preview func, which draws a value card instead of the usual body.
pub(crate) fn is_func(func_id: FuncId) -> bool {
    func_id == PREVIEW_FUNC_ID
}

#[cfg(test)]
mod tests;
