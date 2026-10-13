//! [`FuncOutput`]: one output port a func declares, and its [`OutputType`].

use crate::DataType;

/// An output port's type: either a fixed [`DataType`], or a *wildcard* that
/// mirrors an input. A sum type (rather than a `DataType` + an
/// `Option<mirror>`) so a wildcard can't carry a stray concrete type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputType {
    /// A fixed, declared output type.
    Fixed(DataType),
    /// A polymorphic passthrough / reroute output whose type mirrors the
    /// resolved type of input `mirrors`. It reads as
    /// the wildcard `Any` until the editor resolves it by following the wire
    /// (see [`OutputTypes`](crate::OutputTypes));
    /// compilation resolves the same type into the cache signature and codec preflight.
    Wildcard { mirrors: usize },
}

impl OutputType {
    /// The fixed type, or `Any` for an (unresolved) wildcard — the declared
    /// fallback the editor shows before resolving the wire.
    pub fn declared(&self) -> DataType {
        match self {
            Self::Fixed(ty) => ty.clone(),
            Self::Wildcard { .. } => DataType::Any,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncOutput {
    pub name: String,
    pub ty: OutputType,
    /// One-line human explanation shown as the port's hover tooltip in the
    /// editor. Display-only — execution never reads it.
    pub description: Option<String>,
}

impl FuncOutput {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            ty: OutputType::Fixed(data_type),
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
