//! Generic struct introspection.
//!
//! Describe a struct's fields — name, label, [`FieldKind`], default, required —
//! and rebuild it from neutral [`FieldValue`]s, with **no** coupling to any GUI
//! toolkit or value model. A consumer (e.g. an editor) maps [`FieldDesc`] to its
//! own widgets and reads edited values back as `FieldValue`s.
//!
//! Derive with `#[derive(Introspect)]` (re-exported here). Enum-typed fields
//! must implement [`IntrospectEnum`] (variant list + string round-trip), e.g.
//! via `strum`.
//!
//! ```ignore
//! #[derive(Default, Introspect)]
//! struct Knobs { tile_size: usize, #[config(label = "σ")] sigma: f32 }
//! let fields = Knobs::fields();              // [{name:"tile_size", kind:Int, ..}, ..]
//! let knobs = Knobs::from_fields(&values);   // typed rebuild
//! ```

/// A neutral scalar value carried in/out of introspection.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Int(i64),
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
    Int,
    Float,
    Bool,
    Str,
    Enum {
        /// The enum type's name (a stable identity key for consumers).
        type_name: String,
        variants: Vec<String>,
    },
    /// An optional field of the inner kind (not required).
    Option(Box<FieldKind>),
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
    /// mismatched value falls back to that field's `Default`.
    fn from_fields(values: &[FieldValue]) -> Self;
}

/// A fieldless enum usable as an introspected field — a variant list plus a
/// string round-trip. Trivially backed by `strum` (`EnumIter` + `Display` +
/// `EnumString`).
pub trait IntrospectEnum: Sized {
    fn variants() -> Vec<String>;
    fn to_variant(&self) -> String;
    fn from_variant(name: &str) -> Option<Self>;
}

pub use common_derive::Introspect;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    enum Mode {
        #[default]
        Fast,
        Slow,
    }

    impl IntrospectEnum for Mode {
        fn variants() -> Vec<String> {
            vec!["fast".to_string(), "slow".to_string()]
        }
        fn to_variant(&self) -> String {
            match self {
                Mode::Fast => "fast",
                Mode::Slow => "slow",
            }
            .to_string()
        }
        fn from_variant(name: &str) -> Option<Self> {
            match name {
                "fast" => Some(Mode::Fast),
                "slow" => Some(Mode::Slow),
                _ => None,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Introspect)]
    struct Knobs {
        tile_size: u32,
        #[config(label = "Custom Label")]
        threshold: f32,
        mode: Mode,
        enabled: bool,
        limit: Option<u32>,
    }

    impl Default for Knobs {
        fn default() -> Self {
            Self {
                tile_size: 128,
                threshold: 2.5,
                mode: Mode::Fast,
                enabled: true,
                limit: None,
            }
        }
    }

    #[test]
    fn fields_carry_labels_kinds_required_and_defaults() {
        let fields = Knobs::fields();
        let labels: Vec<&str> = fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Tile Size", "Custom Label", "Mode", "Enabled", "Limit"]
        );
        assert_eq!(fields[0].name, "tile_size");
        assert_eq!(fields[0].kind, FieldKind::Int);
        assert_eq!(fields[1].kind, FieldKind::Float);
        assert_eq!(
            fields[2].kind,
            FieldKind::Enum {
                type_name: "Mode".to_string(),
                variants: vec!["fast".to_string(), "slow".to_string()],
            }
        );
        assert_eq!(fields[3].kind, FieldKind::Bool);
        assert_eq!(fields[4].kind, FieldKind::Option(Box::new(FieldKind::Int)));

        assert!(fields[..4].iter().all(|f| f.required));
        assert!(!fields[4].required, "Option field is optional");

        assert_eq!(fields[0].default, FieldValue::Int(128));
        assert_eq!(fields[2].default, FieldValue::Enum("fast".to_string()));
        assert_eq!(fields[4].default, FieldValue::Null);
    }

    #[test]
    fn rebuilds_from_default_values() {
        let values: Vec<FieldValue> = Knobs::fields().into_iter().map(|f| f.default).collect();
        assert_eq!(Knobs::from_fields(&values), Knobs::default());
    }

    #[test]
    fn rebuilds_with_overrides_and_falls_back() {
        let values = [
            FieldValue::Int(64),
            FieldValue::Float(9.0),
            FieldValue::Enum("slow".to_string()),
            FieldValue::Bool(false),
            FieldValue::Int(7), // Some(7)
        ];
        let knobs = Knobs::from_fields(&values);
        assert_eq!(knobs.tile_size, 64);
        assert_eq!(knobs.mode, Mode::Slow);
        assert_eq!(knobs.limit, Some(7));

        // Mismatched / missing → that field's default.
        let knobs = Knobs::from_fields(&[FieldValue::Bool(true)]);
        assert_eq!(knobs, Knobs::default());
    }
}
