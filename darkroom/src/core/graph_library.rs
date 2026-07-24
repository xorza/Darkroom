//! User-owned reusable graph definitions.

use std::collections::HashMap;

use scenarium::{GraphDef, GraphId};

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct GraphLibrary {
    #[serde(default)]
    pub(crate) graphs: HashMap<GraphId, GraphDef>,
}
