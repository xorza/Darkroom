//! [`InspectTarget`]: which node an inspector tab shows.

use scenarium::NodeId;
use serde::{Deserialize, Serialize};

/// Which node an inspector tab shows: whichever one node is selected, or one node it was pinned
/// to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum InspectTarget {
    /// The one selected node; the tab asks for one when the selection holds none or several.
    Selection,
    /// One node, whatever the selection; the tab closes with the node.
    Node(NodeId),
}
