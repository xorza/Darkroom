//! [`EventRef`]: one event port's identity.

use scenarium::NodeId;

use crate::gui::node_key::NodeKey;

/// One event (emitter) port's identity. Events are indexed independently of data outputs, so they
/// get their own ref rather than a `PortRef` kind. Domain-keyed like
/// [`PortRef`](crate::gui::port_ref::PortRef) so geometry/drag code derives the
/// glyph's `WidgetId` (`event_glyph_wid`) without a cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EventRef {
    pub(crate) node_id: NodeId,
    pub(crate) event_idx: usize,
}

impl NodeKey for EventRef {
    fn node(self) -> NodeId {
        self.node_id
    }
}
