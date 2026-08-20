//! Graph execution + the worker event loop. Commands only request work;
//! worker status reports drive the toolbar's execution and loop state.

use scenarium::NodeId;

use crate::gui::app::App;

/// Graph execution + the worker event loop. Applied by [`RunCommand::apply`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCommand {
    /// Evaluate the graph once on the worker.
    Once,
    /// Evaluate one node's upstream cone and deliver its outputs.
    Node(NodeId),
    /// Remove one authored node's compiled runtime-cache cone from RAM and disk.
    EvictCache(NodeId),
    /// Write one authored node's resident value to the disk store now — raised
    /// when its cache mode gains the disk bit.
    FlushCache(NodeId),
    /// Request cancellation of the in-flight run.
    Cancel,
    /// Start the worker's event loop (emitter events → run subscribers).
    StartEvents,
    /// Stop the worker's event loop.
    StopEvents,
}

impl RunCommand {
    pub(super) fn apply(self, app: &mut App) {
        match self {
            RunCommand::Once => app.run_graph(),
            RunCommand::Node(node_id) => app.run_node(node_id),
            RunCommand::EvictCache(node_id) => app.evict_cache(node_id),
            RunCommand::FlushCache(node_id) => app.flush_cache(node_id),
            RunCommand::Cancel => app.runtime.cancel_run(),
            RunCommand::StartEvents => app.start_events(),
            RunCommand::StopEvents => app.stop_events(),
        }
    }
}
