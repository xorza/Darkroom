//! [`PaneAsk`]: what a command asks of one graph pane.

use scenarium::NodeId;

use crate::gui::frame::framing::Framing;

/// What a command asks of one graph pane, which the pane answers at its next input stage, where
/// it has its geometry, its pointer and its widgets' ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneAsk {
    Frame(Framing),
    /// Open the palette at the pointer, or at the pane's center when the pointer is outside it.
    AddNode,
    /// Open the palette to pick what takes the node's place.
    Replace(NodeId),
    /// Open the editor of the node's title.
    Rename(NodeId),
}
