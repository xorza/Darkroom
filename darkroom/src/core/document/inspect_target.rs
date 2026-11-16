//! [`InspectTarget`]: which node an inspector tab shows.

use scenarium::{InstancePath, NodeAddr};
use serde::{Deserialize, Serialize};

/// Which node an inspector tab shows: whichever one node of the active graph is selected, or one
/// node it was pinned to, with the instance it shows the node's run results through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum InspectTarget {
    /// The one selected node; the tab asks for one when the selection holds none or several.
    Selection,
    /// One node, whatever the selection; the tab closes with the node.
    Node {
        addr: NodeAddr,
        through: Option<InstancePath>,
    },
}
