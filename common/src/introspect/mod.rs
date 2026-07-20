//! Generic struct introspection.
//!
//! Describe a struct's fields — name, label, [`FieldKind`], default, required —
//! and rebuild it from neutral [`FieldValue`]s, with **no** coupling to any GUI
//! toolkit or value model. A consumer (e.g. an editor) maps [`FieldDesc`] to its
//! own widgets and reads edited values back as `FieldValue`s.
//!
//! With Common's `introspect-derive` feature, derive with
//! `#[derive(Introspect)]`. Enum-typed fields implement [`IntrospectEnum`]
//! (variant list + string round-trip) — derive it with
//! `#[derive(IntrospectEnum)]` plus a stable `#[config(type_id = "…")]` UUID;
//! the derive delegates to the enum's `Display`/`FromStr` (typically strum's
//! `Display`/`EnumString`).
//!
//! ```ignore
//! #[derive(Default, Introspect)]
//! struct Knobs { tile_size: usize, #[config(label = "σ")] sigma: f32 }
//! let fields = Knobs::fields();              // [{name:"tile_size", kind:Int, ..}, ..]
//! let knobs = Knobs::from_fields(&values)?;  // checked typed rebuild
//! ```

/// The exact integer type represented by an introspected field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegerKind {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

impl IntegerKind {
    fn type_name(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
            Self::Isize => "isize",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::Usize => "usize",
        }
    }
}

/// The exact floating-point type represented by an introspected field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatKind {
    F32,
    F64,
}

impl FloatKind {
    fn type_name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }
}

/// A lossless neutral integer value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegerValue {
    Signed(i128),
    Unsigned(u128),
}

impl std::fmt::Display for IntegerValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Signed(value) => value.fmt(f),
            Self::Unsigned(value) => value.fmt(f),
        }
    }
}

macro_rules! impl_integer_value {
    (signed: $($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for IntegerValue {
                fn from(value: $ty) -> Self {
                    Self::Signed(value as i128)
                }
            }

            impl TryFrom<IntegerValue> for $ty {
                type Error = ();

                fn try_from(value: IntegerValue) -> Result<Self, Self::Error> {
                    match value {
                        IntegerValue::Signed(value) => Self::try_from(value).map_err(|_| ()),
                        IntegerValue::Unsigned(value) => Self::try_from(value).map_err(|_| ()),
                    }
                }
            }
        )+
    };
    (unsigned: $($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for IntegerValue {
                fn from(value: $ty) -> Self {
                    Self::Unsigned(value as u128)
                }
            }

            impl TryFrom<IntegerValue> for $ty {
                type Error = ();

                fn try_from(value: IntegerValue) -> Result<Self, Self::Error> {
                    match value {
                        IntegerValue::Signed(value) => Self::try_from(value).map_err(|_| ()),
                        IntegerValue::Unsigned(value) => Self::try_from(value).map_err(|_| ()),
                    }
                }
            }
        )+
    };
}

impl_integer_value!(signed: i8, i16, i32, i64, i128, isize);
impl_integer_value!(unsigned: u8, u16, u32, u64, u128, usize);

/// A neutral scalar value carried in/out of introspection.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Int(IntegerValue),
    Float(f64),
    Bool(bool),
    Str(String),
    /// An enum variant, by name (see [`IntrospectEnum`]).
    Enum(String),
    /// Unset — an `Option` field's `None`.
    Null,
}

/// The kind of a reflected field (drives which editor widget a consumer shows).
#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Int(IntegerKind),
    Float(FloatKind),
    Bool,
    Str,
    Enum {
        /// Stable UUID identity, independent of Rust and display names.
        type_id: String,
        display_name: String,
        variants: Vec<String>,
    },
    /// An optional field of the inner kind (not required).
    Option(Box<FieldKind>),
}

/// A numeric field value that cannot be represented by its declared Rust type.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("field `{field}` value {value} cannot be represented as {target}")]
pub struct IntrospectError {
    field: &'static str,
    value: String,
    target: &'static str,
}

impl IntrospectError {
    fn integer(field: &'static str, value: IntegerValue, target: IntegerKind) -> Self {
        Self {
            field,
            value: value.to_string(),
            target: target.type_name(),
        }
    }

    fn float(field: &'static str, value: f64, target: FloatKind) -> Self {
        Self {
            field,
            value: value.to_string(),
            target: target.type_name(),
        }
    }
}

#[doc(hidden)]
pub trait IntrospectInteger: Copy + Into<IntegerValue> + TryFrom<IntegerValue, Error = ()> {
    const KIND: IntegerKind;

    fn from_field_value(field: &'static str, value: IntegerValue) -> Result<Self, IntrospectError> {
        Self::try_from(value).map_err(|()| IntrospectError::integer(field, value, Self::KIND))
    }
}

macro_rules! impl_introspect_integer {
    ($($ty:ty => $kind:ident),+ $(,)?) => {
        $(
            impl IntrospectInteger for $ty {
                const KIND: IntegerKind = IntegerKind::$kind;
            }
        )+
    };
}

impl_introspect_integer!(
    i8 => I8,
    i16 => I16,
    i32 => I32,
    i64 => I64,
    i128 => I128,
    isize => Isize,
    u8 => U8,
    u16 => U16,
    u32 => U32,
    u64 => U64,
    u128 => U128,
    usize => Usize,
);

#[doc(hidden)]
pub trait IntrospectFloat: Copy {
    const KIND: FloatKind;

    fn into_field_value(self) -> f64;
    fn from_field_value(field: &'static str, value: f64) -> Result<Self, IntrospectError>;
}

impl IntrospectFloat for f32 {
    const KIND: FloatKind = FloatKind::F32;

    fn into_field_value(self) -> f64 {
        f64::from(self)
    }

    fn from_field_value(field: &'static str, value: f64) -> Result<Self, IntrospectError> {
        if value.is_finite() && value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX) {
            Ok(value as f32)
        } else {
            Err(IntrospectError::float(field, value, Self::KIND))
        }
    }
}

impl IntrospectFloat for f64 {
    const KIND: FloatKind = FloatKind::F64;

    fn into_field_value(self) -> f64 {
        self
    }

    fn from_field_value(_field: &'static str, value: f64) -> Result<Self, IntrospectError> {
        Ok(value)
    }
}

/// One reflected field.
#[derive(Clone, Debug)]
pub struct FieldDesc {
    /// The Rust field name.
    pub name: String,
    /// Human label (`#[config(label = "...")]` or the name title-cased).
    pub label: String,
    pub kind: FieldKind,
    pub default: FieldValue,
    /// `false` only for `Option<_>` fields.
    pub required: bool,
}

/// A struct whose fields can be described and rebuilt generically.
/// Derive with `#[derive(Introspect)]`.
pub trait Introspect: Default {
    /// Field descriptors in declaration order.
    fn fields() -> Vec<FieldDesc>;
    /// Rebuild from per-field values (declaration order). A missing or
    /// mismatched value falls back to that field's `Default`; a numeric value
    /// outside its concrete field type returns an error.
    fn from_fields(values: &[FieldValue]) -> Result<Self, IntrospectError>;
}

/// A fieldless enum usable as an introspected field — a variant list plus a
/// string round-trip. Derive with `#[derive(IntrospectEnum)]` and a stable
/// `#[config(type_id = "…")]` UUID (needs the enum's `Display` + `FromStr` —
/// e.g. strum's `Display` + `EnumString`).
pub trait IntrospectEnum: Sized {
    const TYPE_ID: &'static str;
    const DISPLAY_NAME: &'static str;

    fn variants() -> Vec<String>;
    fn to_variant(&self) -> String;
    fn from_variant(name: &str) -> Option<Self>;
}

#[cfg(feature = "introspect-derive")]
pub use common_derive::{Introspect, IntrospectEnum};

#[cfg(test)]
mod tests;
