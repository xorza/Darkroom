use crate::runtime::context::ContextType;

use crate::graph::CacheMode;
use crate::node::event::EventLambda;
use crate::node::lambda::FuncLambda;
use crate::{DataType, StaticValue};
use common::id_type;
use serde::{Deserialize, Serialize};

use crate::error::{ValidationError, ensure_valid};

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
    /// Human label shown in the editor's picker dropdown. Display-only — the
    /// value bound on pick is [`ValueVariant::value`], never this. Defaults to
    /// `name` via [`ValueVariant::new`]; override with [`ValueVariant::display`]
    /// to show a friendlier label than a raw/serialized `name`.
    #[serde(default)]
    pub display_name: String,
}

impl ValueVariant {
    /// A picker variant whose dropdown label is its `name`.
    pub fn new(name: impl Into<String>, value: StaticValue) -> Self {
        let name = name.into();
        Self {
            display_name: name.clone(),
            name,
            value,
        }
    }

    /// Override the dropdown label (leaving `name`/`value` untouched).
    pub fn display(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FuncInput {
    pub name: String,
    pub required: bool,
    pub data_type: DataType,
    /// One-line human explanation shown as the port's hover tooltip in the
    /// editor (units, range, meaning). Display-only — execution never reads it.
    #[serde(default)]
    pub description: Option<String>,
    /// When set, this input may only hold a `Const` literal — wiring an upstream
    /// output into it (a `Bind`) is rejected by graph validation and blocked in
    /// the editor. For inputs a node reads as configuration, so a stray
    /// connection can't silently defeat that.
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
            description: None,
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
            description: None,
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

    /// Attach the port's hover-tooltip text. See [`FuncInput::description`].
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
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
    /// resolved type of input `mirrors`. It reads as
    /// the wildcard `Any` until the editor resolves it by following the wire
    /// (see [`Graph::resolve_output_type`](crate::graph::Graph::resolve_output_type));
    /// compilation resolves the same type into the cache signature and codec preflight.
    Wildcard { mirrors: usize },
}

impl OutputType {
    /// The fixed type, or `Any` for an (unresolved) wildcard — the declared
    /// fallback the editor shows before resolving the wire.
    pub fn declared(&self) -> DataType {
        match self {
            OutputType::Fixed(ty) => ty.clone(),
            OutputType::Wildcard { .. } => DataType::Any,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FuncOutput {
    pub name: String,
    pub ty: OutputType,
    /// One-line human explanation shown as the port's hover tooltip in the
    /// editor. Display-only — execution never reads it.
    #[serde(default)]
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
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[derive(Clone, Debug)]
pub struct FuncEvent {
    pub name: String,
    pub event_lambda: EventLambda,
}

id_type!(FuncId);

#[derive(Default, Clone, Debug)]
pub struct Func {
    pub id: FuncId,
    /// Version of this function's output semantics. Folded into pure-node cache keys;
    /// increment it when the implementation can return different values for the same inputs.
    pub version: u32,
    pub name: String,
    pub category: String,
    pub sink: bool,

    /// Node manages its own output caching, so the editor's disk-cache (persist)
    /// toggle is meaningless on it and hidden.
    /// `false` (the default) means a normal node that offers the toggle.
    pub uncacheable: bool,

    /// The [`CacheMode`] a freshly created node of this func copies into its
    /// `cache`. Defaults to [`CacheMode::None`] (no caching); raise it with the
    /// [`default_cache_mode`](Func::default_cache_mode) builder for funcs worth
    /// caching out of the box. Only a policy for *new* nodes — existing nodes
    /// keep whatever mode they were authored/saved with.
    pub default_cache_mode: CacheMode,

    pub behavior: FuncBehavior,

    pub description: Option<String>,
    pub inputs: Vec<FuncInput>,
    pub outputs: Vec<FuncOutput>,
    pub events: Vec<FuncEvent>,
    pub required_contexts: Vec<ContextType>,
    pub lambda: FuncLambda,
}

impl Func {
    /// Start a func definition. Defaults: `Impure`, non-sink, empty
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

    /// Mark the func `Pure` (same inputs → same outputs; cacheable).
    pub fn pure(mut self) -> Self {
        self.behavior = FuncBehavior::Pure;
        self
    }

    pub fn sink(mut self) -> Self {
        self.sink = true;
        self
    }

    /// Hide the editor's disk-cache (persist) toggle for this node — for nodes
    /// that cache their output themselves. See [`Func::uncacheable`].
    pub fn uncacheable(mut self) -> Self {
        self.uncacheable = true;
        self
    }

    /// Set the [`CacheMode`] that new nodes of this func adopt (see
    /// [`Func::default_cache_mode`]). Defaults to [`CacheMode::None`]; raise it
    /// for funcs whose output is worth caching by default.
    pub fn default_cache_mode(mut self, mode: CacheMode) -> Self {
        self.default_cache_mode = mode;
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

    /// Add an output port. Build it with [`FuncOutput::new`], optionally chaining
    /// [`FuncOutput::description`] — mirrors the [`Func::input`] +
    /// [`FuncInput`] builder pattern.
    pub fn output(mut self, output: FuncOutput) -> Self {
        self.outputs.push(output);
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
            description: None,
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

    /// Validates this function declaration independently of a graph.
    pub fn validate(&self) -> Result<(), ValidationError> {
        ensure_valid!(!self.id.is_nil(), "function id must not be nil");
        ensure_valid!(
            !self.outputs.is_empty() || self.behavior == FuncBehavior::Impure,
            "Function with no outputs should be impure"
        );
        for (input_idx, input) in self.inputs.iter().enumerate() {
            if let DataType::Custom(type_id) | DataType::Enum(type_id) = &input.data_type {
                ensure_valid!(
                    !type_id.is_nil(),
                    "function {:?} input {input_idx} has a nil nominal type id",
                    self.id
                );
            }
        }
        for (output_idx, output) in self.outputs.iter().enumerate() {
            match &output.ty {
                OutputType::Fixed(DataType::Custom(type_id) | DataType::Enum(type_id)) => {
                    ensure_valid!(
                        !type_id.is_nil(),
                        "function {:?} output {output_idx} has a nil nominal type id",
                        self.id
                    );
                }
                OutputType::Wildcard { mirrors } => {
                    ensure_valid!(
                        *mirrors < self.inputs.len(),
                        "function {:?} output {output_idx} mirrors input {mirrors}, but has {} inputs",
                        self.id,
                        self.inputs.len()
                    );
                }
                OutputType::Fixed(_) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::graph::CacheMode;
    use crate::node::definition::{Func, FuncId, FuncInput, FuncOutput};
    use crate::{DataType, TypeId};

    #[test]
    fn validate_rejects_invalid_identities_and_wildcard_indices() {
        let nil_input = Func::new(FuncId::unique(), "input").input(FuncInput::required(
            "value",
            DataType::Custom(TypeId::nil()),
        ));
        let nil_output = Func::new(FuncId::unique(), "output")
            .output(FuncOutput::new("value", DataType::Enum(TypeId::nil())));
        let invalid_wildcard = Func::new(FuncId::unique(), "wildcard")
            .input(FuncInput::required("value", DataType::Any))
            .wildcard_output("value", 1);
        let invalid = [
            (
                "function id must not be nil".to_owned(),
                Func::new(FuncId::nil(), "nil"),
            ),
            (
                format!(
                    "function {:?} input 0 has a nil nominal type id",
                    nil_input.id
                ),
                nil_input,
            ),
            (
                format!(
                    "function {:?} output 0 has a nil nominal type id",
                    nil_output.id
                ),
                nil_output,
            ),
            (
                format!(
                    "function {:?} output 0 mirrors input 1, but has 1 inputs",
                    invalid_wildcard.id
                ),
                invalid_wildcard,
            ),
        ];

        for (expected, func) in invalid {
            assert_eq!(func.validate().unwrap_err().to_string(), expected);
        }

        Func::new(FuncId::unique(), "wildcard")
            .input(FuncInput::required("value", DataType::Any))
            .wildcard_output("value", 0)
            .validate()
            .unwrap();
    }

    #[test]
    fn default_cache_mode_defaults_to_none_and_builder_overrides() {
        // Out of the box a func caches nothing — both `Func::default()` and the
        // `Func::new` builder start at `CacheMode::None`.
        assert_eq!(Func::default().default_cache_mode, CacheMode::None);
        assert_eq!(
            Func::new(FuncId::unique(), "f").default_cache_mode,
            CacheMode::None
        );

        // The builder sets a hotter default; distinct inputs map to distinct
        // stored modes (not a fixed constant).
        for mode in [CacheMode::Ram, CacheMode::Disk, CacheMode::Both] {
            let func = Func::new(FuncId::unique(), "f").default_cache_mode(mode);
            assert_eq!(func.default_cache_mode, mode, "{mode:?} is stored verbatim");
        }
    }
}
