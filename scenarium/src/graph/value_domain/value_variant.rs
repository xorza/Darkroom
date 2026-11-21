//! [`ValueVariant`]: one entry of an input's editor picker.

use serde::{Deserialize, Serialize};

use crate::data::const_value::ConstValue;

/// One entry of an input's picker: the label it shows, and the constant a pick binds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueVariant {
    pub label: String,
    pub value: ConstValue,
}

impl ValueVariant {
    pub fn new(label: impl Into<String>, value: ConstValue) -> Self {
        Self {
            label: label.into(),
            value,
        }
    }
}
