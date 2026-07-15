use common::KeyIndexKey;
use glam::Vec2;
use scenarium::{NodeId, OutputPort};
use serde::{Deserialize, Serialize};

use crate::core::document::ItemRef;

/// One canvas item's persisted view state — a node body's position or a
/// pinned output's preview-widget position. Lives in
/// [`crate::core::document::GraphView::view_items`], whose *order* is the
/// shared paint stack for both kinds (later = frontmost).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ViewItem {
    pub key: ItemRef,
    pub pos: Vec2,
}

impl Eq for ViewItem {}

impl ViewItem {
    pub(crate) fn node(id: NodeId, pos: Vec2) -> Self {
        Self {
            key: ItemRef::Node(id),
            pos,
        }
    }

    pub(crate) fn pin(port: OutputPort, pos: Vec2) -> Self {
        Self {
            key: ItemRef::Pin(port),
            pos,
        }
    }
}

impl KeyIndexKey<ItemRef> for ViewItem {
    fn key(&self) -> &ItemRef {
        &self.key
    }
}
