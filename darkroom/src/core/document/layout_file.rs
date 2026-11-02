//! The pane layout as a document file holds it.
//!
//! - **The file holds palantir's `DockLayout` alone, loaded back with the graph tab pinned.**
//!   The pinned tab is the editor's, so no file can name another. A layout palantir's load
//!   refuses fails the decode, as a corrupt tree did before.

use palantir::{DockLayout, DockState};
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
    DockState::load(DockLayout::deserialize(deserializer)?, TabRef::Graph).map_err(D::Error::custom)
}
