//! [`FuncOutput`]: one output port a func declares.

use crate::data::data_type::DataType;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncOutput {
    pub name: String,
    pub data_type: DataType,
    /// One-line human explanation shown as the port's hover tooltip in the
    /// editor. Display-only — execution never reads it.
    pub description: Option<String>,
}

impl FuncOutput {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            description: None,
        }
    }

    /// Attach the port's hover-tooltip text. See [`FuncOutput::description`].
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}
