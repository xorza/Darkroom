//! Generic struct introspection.
//!
//! Describe a struct's fields — name, label, [`FieldKind`], default, required —
//! and rebuild it from neutral [`FieldValue`]s, with **no** coupling to any GUI
//! toolkit or value model. A consumer (e.g. an editor) maps [`FieldDesc`] to its
//! own widgets and reads edited values back as `FieldValue`s.
//!
//! Derive with `#[derive(Introspect)]` (re-exported here). Enum-typed fields
//! implement [`IntrospectEnum`] (variant list + string round-trip) — derive it
//! with `#[derive(IntrospectEnum)]` plus a stable `#[config(type_id = "…")]`
//! UUID; the derive delegates to the enum's `Display`/`FromStr` (typically
//! strum's `Display`/`EnumString`).
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
        /// Stable UUID identity, independent of Rust and display names.
        type_id: String,
        display_name: String,
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

pub use common_derive::{Introspect, IntrospectEnum};

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
        const TYPE_ID: &'static str = "8254c974-43ba-4bd4-9521-6dd749aab5ea";
        const DISPLAY_NAME: &'static str = "Mode";

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

    /// `#[derive(IntrospectEnum)]` — proves the derive needs only `Display` +
    /// `FromStr` (hand-written here; strum's `Display`/`EnumString` are the usual
    /// source), with no `strum`/`IntoEnumIterator` dependency.
    #[derive(Debug, Clone, Copy, PartialEq, IntrospectEnum)]
    #[config(type_id = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb")]
    enum Speed {
        Fast,
        Slow,
    }

    impl std::fmt::Display for Speed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(match self {
                Speed::Fast => "fast",
                Speed::Slow => "slow",
            })
        }
    }

    impl std::str::FromStr for Speed {
        type Err = ();
        fn from_str(s: &str) -> Result<Self, ()> {
            match s {
                "fast" => Ok(Speed::Fast),
                "slow" => Ok(Speed::Slow),
                _ => Err(()),
            }
        }
    }

    #[test]
    fn derived_introspect_enum_delegates_to_display_and_from_str() {
        assert_ne!(Speed::TYPE_ID, Mode::TYPE_ID);
        assert_eq!(Speed::DISPLAY_NAME, "Speed");
        assert_eq!(Speed::variants(), ["fast", "slow"]);
        assert_eq!(Speed::Slow.to_variant(), "slow");
        assert_eq!(Speed::from_variant("fast"), Some(Speed::Fast));
        assert_eq!(Speed::from_variant("nope"), None);
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

    #[derive(Debug, Clone, PartialEq, Introspect)]
    struct OptionalDefaults {
        default_none: Option<u32>,
        default_some: Option<u32>,
    }

    impl Default for OptionalDefaults {
        fn default() -> Self {
            Self {
                default_none: None,
                default_some: Some(11),
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
                type_id: Mode::TYPE_ID.to_string(),
                display_name: "Mode".to_string(),
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

        assert_eq!(
            OptionalDefaults::from_fields(&[]),
            OptionalDefaults::default()
        );
        assert_eq!(
            OptionalDefaults::from_fields(&[FieldValue::Null, FieldValue::Null]),
            OptionalDefaults {
                default_none: None,
                default_some: None,
            }
        );
        assert_eq!(
            OptionalDefaults::from_fields(&[FieldValue::Int(7), FieldValue::Int(13)]),
            OptionalDefaults {
                default_none: Some(7),
                default_some: Some(13),
            }
        );
        assert_eq!(
            OptionalDefaults::from_fields(&[
                FieldValue::Bool(false),
                FieldValue::Str("wrong".to_string()),
            ]),
            OptionalDefaults::default()
        );
    }

    mod other {
        use crate::IntrospectEnum;

        #[derive(Debug)]
        pub(crate) enum Mode {
            Only,
        }

        impl IntrospectEnum for Mode {
            const TYPE_ID: &'static str = "b3ee5042-6965-4d47-a8ca-bcd979dd5491";
            const DISPLAY_NAME: &'static str = "Mode";

            fn variants() -> Vec<String> {
                vec!["only".to_string()]
            }

            fn to_variant(&self) -> String {
                match self {
                    Mode::Only => "only".to_string(),
                }
            }

            fn from_variant(name: &str) -> Option<Self> {
                (name == "only").then_some(Mode::Only)
            }
        }
    }

    #[test]
    fn same_named_enums_in_different_modules_have_distinct_identities() {
        assert_eq!(Mode::DISPLAY_NAME, other::Mode::DISPLAY_NAME);
        assert_ne!(Mode::TYPE_ID, other::Mode::TYPE_ID);
        assert_eq!(other::Mode::variants(), ["only"]);
    }
}
