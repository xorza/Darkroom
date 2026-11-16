//! [`TabRef`]: what an editor tab shows.

use scenarium::{GraphId, ProgramId};
use serde::{Deserialize, Serialize};

use crate::core::document::inspect_target::InspectTarget;

/// What an editor tab shows: one of the document's graphs ([`TabRef::Graph`]), or a non-graph app
/// view like [`TabRef::Preferences`], an image viewer or an inspector. Persisted with the
/// document, so reopening one restores its open tabs; not undoable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TabRef {
    /// A graph's pane.
    Graph(GraphId),
    /// The app-preferences view.
    Preferences,
    /// A full-resolution viewer of one preview program node's runtime image, one tab per program
    /// node. Content is runtime-only (`crate::gui::pane::image_viewer`): a restored tab pulls any
    /// current value from `RunState` when drawn. Pruned when its path stops naming a preview.
    ImageViewer(ProgramId),
    /// A node's details at UI scale: its status, inputs, outputs and log. Content is
    /// runtime-only; a pinned one is pruned with its node.
    Inspector(InspectTarget),
}

impl TabRef {
    /// The graph this tab shows in a canvas, or `None` for a tab of another kind.
    pub(crate) const fn graph(self) -> Option<GraphId> {
        match self {
            Self::Graph(graph) => Some(graph),
            Self::Preferences | Self::ImageViewer(_) | Self::Inspector(_) => None,
        }
    }
}
