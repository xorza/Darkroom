//! The pane layout as a document file holds it.
//!
//! - **The file holds palantir's `DockLayout` alone, loaded back with the main graph's tab pinned.**
//!   The pinned tab is the editor's, so no file can name another. A layout palantir's load
//!   refuses fails the decode.

use palantir::{DockLayout, DockState};
use scenarium::GraphId;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::core::document::tab_ref::TabRef;

pub(super) fn serialize<S: Serializer>(
    layout: &DockState<TabRef>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    layout.layout().serialize(serializer)
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<DockState<TabRef>, D::Error> {
    DockState::load(
        DockLayout::deserialize(deserializer)?,
        TabRef::Graph(GraphId::Main),
    )
    .map_err(D::Error::custom)
}
