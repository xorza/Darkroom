use crate::runtime::context::ContextType;

use crate::data::*;
use crate::node::event_lambda::EventLambda;
use crate::node::func_lambda::FuncLambda;
use common::KeyIndexKey;
use common::id_type;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FuncBehavior {
    // could return different values for same inputs
    #[default]
    Impure,
    // always returns the same value for same inputs
    Pure,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValueVariant {
    pub name: String,
    pub value: StaticValue,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FuncInput {
    pub name: String,
    pub required: bool,
    pub data_type: DataType,
    /// When set, this input may only hold a `Const` literal — wiring an upstream
    /// output into it (a `Bind`) is rejected by graph validation and blocked in
    /// the editor. For inputs the engine reads as a constant (e.g. the file-cache
    /// node's `path`), so a stray connection can't silently defeat that.
    #[serde(default)]
    pub const_only: bool,
    #[serde(default)]
    pub default_value: Option<StaticValue>,
    #[serde(default)]
    pub value_variants: Vec<ValueVariant>,
}

impl FuncInput {
    /// A required input of `data_type` (no const default).
    pub fn required(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            required: true,
            data_type,
            const_only: false,
            default_value: None,
            value_variants: Vec::new(),
        }
    }

    /// An optional input of `data_type`; chain [`Self::default`] to seed a
    /// const default value.
    pub fn optional(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            required: false,
            data_type,
            const_only: false,
            default_value: None,
            value_variants: Vec::new(),
        }
    }

    /// Seed this input's const default value.
    pub fn default(mut self, value: impl Into<StaticValue>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    /// Restrict this input to a `Const` literal — no upstream `Bind`. See
    /// [`FuncInput::const_only`].
    pub fn const_only(mut self) -> Self {
        self.const_only = true;
        self
    }

    /// Attach the editor picker variants (`ValueVariant`s).
    pub fn variants(mut self, variants: Vec<ValueVariant>) -> Self {
        self.value_variants = variants;
        self
    }
}

/// An output port's type: either a fixed [`DataType`], or a *wildcard* that
/// mirrors an input. A sum type (rather than a `DataType` + an
/// `Option<mirror>`) so a wildcard can't carry a stray concrete type.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputType {
    /// A fixed, declared output type.
    Fixed(DataType),
    /// A polymorphic passthrough / reroute output whose type mirrors the
    /// resolved type of input `mirrors` (e.g. the file-cache node). It reads as
    /// the wildcard `Null` until the editor resolves it by following the wire
    /// (see [`Graph::resolve_output_type`](crate::graph::Graph::resolve_output_type));
    /// the engine never type-checks, so it ignores the distinction.
    Wildcard { mirrors: usize },
}

impl OutputType {
    /// The fixed type, or `Null` for an (unresolved) wildcard — the declared
    /// fallback the editor shows before resolving the wire.
    pub fn declared(&self) -> DataType {
        match self {
            OutputType::Fixed(ty) => ty.clone(),
            OutputType::Wildcard { .. } => DataType::Null,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FuncOutput {
    pub name: String,
    pub ty: OutputType,
}

impl FuncOutput {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            ty: OutputType::Fixed(data_type),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FuncEvent {
    pub name: String,

    #[serde(skip, default)]
    pub event_lambda: EventLambda,
}

id_type!(FuncId);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Func {
    pub id: FuncId,
    pub name: String,
    pub category: String,
    pub terminal: bool,

    /// Node manages its own output caching, so the editor's disk-cache (persist)
    /// toggle is meaningless on it and hidden — e.g. the file-cache passthrough,
    /// whose explicit-path store supersedes the generic content-addressed cache.
    /// `false` (the default) means a normal node that offers the toggle.
    #[serde(default)]
    pub uncacheable: bool,

    pub behavior: FuncBehavior,

    /// Algorithm version, folded into the disk-cache content digest so a changed
    /// implementation invalidates results computed by an older binary. Bump it
    /// whenever the func's output for identical inputs changes. Pure cache
    /// metadata — execution never reads it.
    #[serde(default)]
    pub version: u64,

    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub inputs: Vec<FuncInput>,
    #[serde(default)]
    pub outputs: Vec<FuncOutput>,
    #[serde(default)]
    pub events: Vec<FuncEvent>,
    #[serde(skip, default)]
    pub required_contexts: Vec<ContextType>,

    #[serde(skip, default)]
    pub lambda: FuncLambda,
}

impl KeyIndexKey<FuncId> for Func {
    fn key(&self) -> &FuncId {
        &self.id
    }
}

impl Func {
    /// Start a func definition. Defaults: `Impure`, non-terminal, empty
    /// category/inputs/outputs/events and a `None` lambda — set the rest with the
    /// chained builders below.
    pub fn new(id: impl Into<FuncId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Stamp the func's algorithm [`version`](Func::version). Bump when the
    /// implementation changes its output for the same inputs, to invalidate
    /// disk-cached results from older binaries.
    pub fn version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    /// Mark the func `Pure` (same inputs → same outputs; cacheable).
    pub fn pure(mut self) -> Self {
        self.behavior = FuncBehavior::Pure;
        self
    }

    pub fn terminal(mut self) -> Self {
        self.terminal = true;
        self
    }

    /// Hide the editor's disk-cache (persist) toggle for this node — for nodes
    /// that cache their output themselves. See [`Func::uncacheable`].
    pub fn uncacheable(mut self) -> Self {
        self.uncacheable = true;
        self
    }

    pub fn input(mut self, input: FuncInput) -> Self {
        self.inputs.push(input);
        self
    }

    pub fn inputs(mut self, inputs: impl IntoIterator<Item = FuncInput>) -> Self {
        self.inputs.extend(inputs);
        self
    }

    pub fn output(mut self, name: impl Into<String>, data_type: DataType) -> Self {
        self.outputs.push(FuncOutput::new(name, data_type));
        self
    }

    /// Add a *wildcard* output that mirrors input `mirrors_input`'s resolved
    /// type — a polymorphic passthrough / reroute port. See
    /// [`OutputType::Wildcard`].
    pub fn wildcard_output(mut self, name: impl Into<String>, mirrors_input: usize) -> Self {
        self.outputs.push(FuncOutput {
            name: name.into(),
            ty: OutputType::Wildcard {
                mirrors: mirrors_input,
            },
        });
        self
    }

    pub fn event(mut self, name: impl Into<String>, event_lambda: EventLambda) -> Self {
        self.events.push(FuncEvent {
            name: name.into(),
            event_lambda,
        });
        self
    }

    pub fn context(mut self, context: ContextType) -> Self {
        self.required_contexts.push(context);
        self
    }

    pub fn lambda(mut self, lambda: FuncLambda) -> Self {
        self.lambda = lambda;
        self
    }

    pub(crate) fn validate(&self) {
        assert!(
            !self.outputs.is_empty() || self.behavior == FuncBehavior::Impure,
            "Function with no outputs should be impure"
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::node::function::Func;
    use common::{SerdeFormat, deserialize};

    #[test]
    fn version_defaults_to_zero_for_legacy_documents() -> anyhow::Result<()> {
        // A document authored before `version` existed carries no such field;
        // `#[serde(default)]` must fill it with 0 rather than fail to parse.
        let legacy = r#"{ "id": "00000000-0000-0000-0000-000000000001", "name": "legacy",
            "category": "", "terminal": false, "behavior": "Impure" }"#;
        let func: Func = deserialize(legacy.as_bytes(), SerdeFormat::Json)?;
        assert_eq!(func.version, 0);
        assert_eq!(Func::default().version, 0);
        Ok(())
    }
}
