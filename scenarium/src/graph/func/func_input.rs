//! [`FuncInput`]: one input port a func declares.

use crate::data::const_value::ConstValue;
use crate::data::data_type::{DataType, Strictness, TypeId};
use crate::graph::func::value_variant::ValueVariant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncInput {
    pub name: String,
    pub required: bool,
    pub data_type: DataType,
    /// One-line human explanation shown as the port's hover tooltip in the
    /// editor (units, range, meaning). Display-only — execution never reads it.
    pub description: Option<String>,
    /// When set, this input may only hold a `Const` literal — wiring an upstream
    /// output into it (a `Wire`) is rejected by graph validation and blocked in
    /// the editor. For inputs a node reads as configuration, so a stray
    /// wire can't silently defeat that.
    pub const_only: bool,
    pub default_value: Option<ConstValue>,
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
    #[must_use]
    pub fn default(mut self, value: impl Into<ConstValue>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    /// Attach the port's hover-tooltip text. See [`FuncInput::description`].
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Restrict this input to a `Const` literal — no upstream `Wire`. See
    /// [`FuncInput::const_only`].
    #[must_use]
    pub const fn const_only(mut self) -> Self {
        self.const_only = true;
        self
    }

    /// Attach the editor picker variants (`ValueVariant`s).
    #[must_use]
    pub fn variants(mut self, variants: Vec<ValueVariant>) -> Self {
        self.value_variants = variants;
        self
    }

    /// Whether `value` may sit on this input as a `Const` — the whole const
    /// gate, both times it is asked.
    ///
    /// The two things a *port* contributes, ahead of the type table in
    /// [`DataType::accepts_const`], are the same at declaration time and at
    /// compile time, which is why they are written once here:
    ///
    /// - a **picker** port (one carrying `value_variants`) takes exactly one
    ///   of the values it offers and nothing else, whatever its declared type
    ///   says — that is what makes it a picker;
    /// - `Null` is "explicitly unset", so it fits an optional port and no
    ///   required one. (lens's config machinery authors it on `Option`-field
    ///   inputs and reads it back as `None`.)
    ///
    /// `strictness` and `enum_variant` are the caller's — see
    /// [`Strictness`].
    pub(crate) fn accepts_const(
        &self,
        value: &ConstValue,
        strictness: Strictness,
        enum_variant: impl FnOnce(TypeId, &str) -> bool,
    ) -> bool {
        if !self.value_variants.is_empty() {
            return self.value_variants.iter().any(|v| v.value == *value);
        }
        if matches!(value, ConstValue::Null) {
            return !self.required;
        }
        self.data_type
            .accepts_const(value, strictness, enum_variant)
    }
}
