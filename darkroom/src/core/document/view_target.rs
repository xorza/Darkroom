//! [`ViewTarget`]: which preview a viewer tab shows.

use scenarium::ProgramId;
use serde::{Deserialize, Serialize};

/// Which preview a viewer tab shows: the document's active one, or one it was pinned to, as
/// [`InspectTarget`](crate::core::document::inspect_target::InspectTarget) follows the selection
/// or holds one node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ViewTarget {
    /// The document's active preview, the one last pressed; the tab asks for one while there is
    /// none.
    Active,
    /// One preview, whatever is active; the tab closes with its node.
    Node(ProgramId),
}
