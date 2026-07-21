//! User-owned reusable graph definitions.

use std::collections::HashMap;

use scenarium::{Graph, GraphId};

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct GraphLibrary {
    #[serde(default)]
    pub(crate) graphs: HashMap<GraphId, Graph>,
}
