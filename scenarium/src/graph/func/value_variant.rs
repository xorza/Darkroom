//! [`ValueVariant`]: one entry of an input's editor picker.

use crate::ConstValue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueVariant {
    pub name: String,
    pub value: ConstValue,
    /// A friendlier dropdown label than `name`, when there is one. See
    /// [`label`](Self::label).
    pub display: Option<String>,
}

impl ValueVariant {
    /// A picker variant whose dropdown label is its `name`.
    pub fn new(name: impl Into<String>, value: ConstValue) -> Self {
        Self {
            name: name.into(),
            value,
            display: None,
        }
    }

    /// Override the dropdown label (leaving `name`/`value` untouched).
    #[must_use]
    pub fn display(mut self, label: impl Into<String>) -> Self {
        self.display = Some(label.into());
        self
    }

    /// The label the editor's picker shows: the display label, else `name`.
    /// Display-only — the value bound on pick is [`value`](Self::value).
    pub fn label(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.name)
    }
}
