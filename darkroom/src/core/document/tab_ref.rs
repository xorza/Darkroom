//! [`TabRef`]: what an editor tab shows, and where it opens.
//!
//! - **A tab has a home.** A graph, Preferences and the reference open among the graphs; a
//!   viewer and an inspector in a sidebar at their right, so the graph they serve stays in view,
//!   as Substance Designer docks its 2D View and its Properties beside its graph
//!   ([interface](https://helpx.adobe.com/substance-3d-designer/using/interface-overview.html)).
//!   Refused: every tab in the focused group, which covered the graph with its own viewer.

use scenarium::GraphId;
use serde::{Deserialize, Serialize};

use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::view_target::ViewTarget;

/// What an editor tab shows: one of the document's graphs ([`TabRef::Graph`]), or a non-graph app
/// view like [`TabRef::Preferences`], an image viewer or an inspector. Persisted with the
/// document, so reopening one restores its open tabs; not undoable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TabRef {
    /// A graph's pane.
    Graph(GraphId),
    /// The app-preferences view.
    Preferences,
    /// A full-resolution viewer of a preview's runtime image: the active preview's, or one it was
    /// pinned to. Content is runtime-only (`crate::gui::pane::image_viewer`): a restored tab
    /// pulls any current value from `RunState` when drawn. A pinned one is pruned when its path
    /// stops naming a preview.
    Viewer(ViewTarget),
    /// A node's details at UI scale: its status, inputs, outputs and log. Content is
    /// runtime-only; a pinned one is pruned with its node.
    Inspector(InspectTarget),
    /// The commands and the canvas's gestures, each with its keys.
    Reference,
}

impl TabRef {
    /// The graph this tab shows in a canvas, or `None` for a tab of another kind.
    pub(crate) const fn graph(self) -> Option<GraphId> {
        match self {
            Self::Graph(graph) => Some(graph),
            Self::Preferences | Self::Viewer(_) | Self::Inspector(_) | Self::Reference => None,
        }
    }
}

/// Where a tab opens when no pane shows it yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabHome {
    /// Among the graph panes: the group that holds the main graph's tab.
    Graphs,
    /// Beside the graphs, at their right.
    Sidebar,
}

impl TabRef {
    /// Where the tab opens: a viewer and an inspector in the sidebar, the rest among the graphs.
    pub(crate) const fn home(self) -> TabHome {
        match self {
            Self::Viewer(_) | Self::Inspector(_) => TabHome::Sidebar,
            Self::Graph(_) | Self::Preferences | Self::Reference => TabHome::Graphs,
        }
    }
}
