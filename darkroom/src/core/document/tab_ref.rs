//! [`TabRef`]: what an editor tab shows.

use scenarium::NodeId;
use serde::{Deserialize, Serialize};

use crate::core::document::inspect_target::InspectTarget;

/// What an editor tab shows: the document's graph ([`TabRef::Graph`]), or a
/// non-graph app view like [`TabRef::Preferences`] (the settings window), an
/// image viewer or an inspector. Persisted with the rest of the document, so reopening one
/// restores its open tabs; not undoable, since the pane arrangement records no
/// undo step, so Ctrl+Z walks past a tab switch to the last graph edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TabRef {
    /// The graph pane.
    Graph,
    /// The app-preferences / settings view — no graph, no canvas.
    Preferences,
    /// A full-resolution viewer of one preview node's runtime image — one tab
    /// per node, deduped on open. Content is runtime-only
    /// (`crate::gui::pane::image_viewer`): a restored tab pulls any current value
    /// from `RunState` when drawn. Pruned when its node is deleted.
    ///
    /// Keyed by the preview node whose value it shows — the same identity the
    /// preview card on the canvas uses, so the two can never disagree about
    /// which value a tab is for.
    ImageViewer(NodeId),
    /// A node's details at UI scale: its status, inputs, outputs and log.
    /// Content is runtime-only, like a viewer's. A pinned one is pruned with its
    /// node.
    Inspector(InspectTarget),
}
