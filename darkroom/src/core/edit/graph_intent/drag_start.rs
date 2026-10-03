//! Where one member of a drag sat when the drag latched.

use glam::Vec2;
use scenarium::NodeId;

/// One member of a group drag, and its position when the pointer latched.
/// A drag frame moves every member to `pos + offset`, measured from here
/// rather than from the frame before, so a dropped frame cannot drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DragStart {
    pub(crate) node: NodeId,
    pub(crate) pos: Vec2,
}
