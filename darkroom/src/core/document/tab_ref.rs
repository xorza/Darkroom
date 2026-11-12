//! [`TabRef`]: what an editor tab shows.

use scenarium::NodeId;
use serde::{Deserialize, Serialize};

use crate::core::document::inspect_target::InspectTarget;

/// What an editor tab shows: the document's graph ([`TabRef::Graph`]), or a non-graph app view
/// like [`TabRef::Preferences`], an image viewer or an inspector. Persisted with the document, so
/// reopening one restores its open tabs; not undoable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TabRef {
    /// The graph pane.
    Graph,
    /// The app-preferences view.
    Preferences,
    /// A full-resolution viewer of one preview node's runtime image, one tab per node. Content is
    /// runtime-only (`crate::gui::pane::image_viewer`): a restored tab pulls any current value
    /// from `RunState` when drawn. Pruned when its node is deleted.
    ImageViewer(NodeId),
    /// A node's details at UI scale: its status, inputs, outputs and log. Content is
    /// runtime-only; a pinned one is pruned with its node.
    Inspector(InspectTarget),
}
