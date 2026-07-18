use crate::introspect::{
    FieldKind, FieldValue, FloatKind, IntegerKind, IntegerValue, Introspect, IntrospectEnum,
    IntrospectFloat, IntrospectInteger,
};

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

    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

#[derive(Debug, PartialEq, Introspect)]
struct WideDefaults {
    signed: i128,
    unsigned: u128,
    pointer_signed: isize,
    pointer_unsigned: usize,
    float: f32,
}

impl Default for WideDefaults {
    fn default() -> Self {
        Self {
            signed: i128::MIN,
            unsigned: u128::MAX,
            pointer_signed: isize::MIN,
            pointer_unsigned: usize::MAX,
            float: f32::MAX,
        }
    }
}

#[derive(Debug, Default, Introspect)]
struct NumericKinds {
    i8_value: i8,
    i16_value: i16,
    i32_value: i32,
    i64_value: i64,
    i128_value: i128,
    isize_value: isize,
    u8_value: u8,
    u16_value: u16,
    u32_value: u32,
    u64_value: u64,
    u128_value: u128,
    usize_value: usize,
    f32_value: f32,
    f64_value: f64,
}

#[test]
fn fields_carry_labels_concrete_kinds_required_and_defaults() {
    let fields = Knobs::fields();
    let labels: Vec<&str> = fields.iter().map(|f| f.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Tile Size", "Custom Label", "Mode", "Enabled", "Limit"]
    );
    assert_eq!(fields[0].name, "tile_size");
    assert_eq!(fields[0].kind, FieldKind::Int(IntegerKind::U32));
    assert_eq!(fields[1].kind, FieldKind::Float(FloatKind::F32));
    assert_eq!(
        fields[2].kind,
        FieldKind::Enum {
            type_id: Mode::TYPE_ID.to_string(),
            display_name: "Mode".to_string(),
            variants: vec!["fast".to_string(), "slow".to_string()],
        }
    );
    assert_eq!(fields[3].kind, FieldKind::Bool);
    assert_eq!(
        fields[4].kind,
        FieldKind::Option(Box::new(FieldKind::Int(IntegerKind::U32)))
    );

    assert!(fields[..4].iter().all(|f| f.required));
    assert!(!fields[4].required, "Option field is optional");

    assert_eq!(
        fields[0].default,
        FieldValue::Int(IntegerValue::Unsigned(128))
    );
    assert_eq!(fields[2].default, FieldValue::Enum("fast".to_string()));
    assert_eq!(fields[4].default, FieldValue::Null);
}

#[test]
fn derive_records_every_concrete_numeric_kind() {
    let actual: Vec<FieldKind> = NumericKinds::fields()
        .into_iter()
        .map(|field| field.kind)
        .collect();
    assert_eq!(
        actual,
        [
            FieldKind::Int(IntegerKind::I8),
            FieldKind::Int(IntegerKind::I16),
            FieldKind::Int(IntegerKind::I32),
            FieldKind::Int(IntegerKind::I64),
            FieldKind::Int(IntegerKind::I128),
            FieldKind::Int(IntegerKind::Isize),
            FieldKind::Int(IntegerKind::U8),
            FieldKind::Int(IntegerKind::U16),
            FieldKind::Int(IntegerKind::U32),
            FieldKind::Int(IntegerKind::U64),
            FieldKind::Int(IntegerKind::U128),
            FieldKind::Int(IntegerKind::Usize),
            FieldKind::Float(FloatKind::F32),
            FieldKind::Float(FloatKind::F64),
        ]
    );
}

#[test]
fn lossless_defaults_round_trip_at_wide_boundaries() {
    let values: Vec<FieldValue> = WideDefaults::fields()
        .into_iter()
        .map(|field| field.default)
        .collect();
    assert_eq!(
        values,
        [
            FieldValue::Int(IntegerValue::Signed(i128::MIN)),
            FieldValue::Int(IntegerValue::Unsigned(u128::MAX)),
            FieldValue::Int(IntegerValue::Signed(isize::MIN as i128)),
            FieldValue::Int(IntegerValue::Unsigned(usize::MAX as u128)),
            FieldValue::Float(f64::from(f32::MAX)),
        ]
    );
    assert_eq!(
        WideDefaults::from_fields(&values).unwrap(),
        WideDefaults::default()
    );
}

macro_rules! assert_signed_boundaries {
    ($($ty:ty => $kind:ident),+ $(,)?) => {
        $(
            assert_eq!(<$ty as IntrospectInteger>::KIND, IntegerKind::$kind);
            assert_eq!(
                <$ty as IntrospectInteger>::from_field_value(
                    "value",
                    IntegerValue::Signed(<$ty>::MIN as i128),
                ),
                Ok(<$ty>::MIN),
            );
            assert_eq!(
                <$ty as IntrospectInteger>::from_field_value(
                    "value",
                    IntegerValue::Signed(<$ty>::MAX as i128),
                ),
                Ok(<$ty>::MAX),
            );
            assert_eq!(
                <$ty as IntrospectInteger>::from_field_value(
                    "value",
                    IntegerValue::Unsigned(<$ty>::MAX as u128),
                ),
                Ok(<$ty>::MAX),
            );
        )+
    };
}

macro_rules! assert_unsigned_boundaries {
    ($($ty:ty => $kind:ident),+ $(,)?) => {
        $(
            assert_eq!(<$ty as IntrospectInteger>::KIND, IntegerKind::$kind);
            assert_eq!(
                <$ty as IntrospectInteger>::from_field_value(
                    "value",
                    IntegerValue::Signed(0),
                ),
                Ok(<$ty>::MIN),
            );
            assert_eq!(
                <$ty as IntrospectInteger>::from_field_value(
                    "value",
                    IntegerValue::Unsigned(<$ty>::MAX as u128),
                ),
                Ok(<$ty>::MAX),
            );
        )+
    };
}

#[test]
fn integer_conversions_accept_every_exact_minimum_and_maximum() {
    assert_signed_boundaries!(
        i8 => I8,
        i16 => I16,
        i32 => I32,
        i64 => I64,
        i128 => I128,
        isize => Isize,
    );
    assert_unsigned_boundaries!(
        u8 => U8,
        u16 => U16,
        u32 => U32,
        u64 => U64,
        u128 => U128,
        usize => Usize,
    );
}

#[test]
fn integer_conversions_reject_values_outside_the_target_domain() {
    for error in [
        <u8 as IntrospectInteger>::from_field_value("value", IntegerValue::Signed(-1)).unwrap_err(),
        <usize as IntrospectInteger>::from_field_value("value", IntegerValue::Signed(-1))
            .unwrap_err(),
        <i8 as IntrospectInteger>::from_field_value("value", IntegerValue::Signed(-129))
            .unwrap_err(),
        <i8 as IntrospectInteger>::from_field_value("value", IntegerValue::Unsigned(128))
            .unwrap_err(),
        <u8 as IntrospectInteger>::from_field_value("value", IntegerValue::Unsigned(256))
            .unwrap_err(),
        <i128 as IntrospectInteger>::from_field_value(
            "value",
            IntegerValue::Unsigned(i128::MAX as u128 + 1),
        )
        .unwrap_err(),
    ] {
        assert!(error.to_string().starts_with("field `value` value "));
    }
}

#[test]
fn float_conversions_check_f32_finite_range_and_preserve_f64() {
    assert_eq!(
        <f32 as IntrospectFloat>::from_field_value("value", f64::from(f32::MIN)),
        Ok(f32::MIN)
    );
    assert_eq!(
        <f32 as IntrospectFloat>::from_field_value("value", f64::from(f32::MAX)),
        Ok(f32::MAX)
    );
    for value in [
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NAN,
        f64::from(f32::MIN) * 2.0,
        f64::from(f32::MAX) * 2.0,
    ] {
        assert!(<f32 as IntrospectFloat>::from_field_value("value", value).is_err());
    }
    assert_eq!(
        <f64 as IntrospectFloat>::from_field_value("value", f64::MIN),
        Ok(f64::MIN)
    );
    assert_eq!(
        <f64 as IntrospectFloat>::from_field_value("value", f64::MAX),
        Ok(f64::MAX)
    );
}

#[test]
fn rebuilds_with_overrides_fallbacks_and_checked_numeric_errors() {
    let values = [
        FieldValue::Int(IntegerValue::Signed(64)),
        FieldValue::Float(9.0),
        FieldValue::Enum("slow".to_string()),
        FieldValue::Bool(false),
        FieldValue::Int(IntegerValue::Signed(7)),
    ];
    let knobs = Knobs::from_fields(&values).unwrap();
    assert_eq!(knobs.tile_size, 64);
    assert_eq!(knobs.threshold, 9.0);
    assert_eq!(knobs.mode, Mode::Slow);
    assert!(!knobs.enabled);
    assert_eq!(knobs.limit, Some(7));

    assert_eq!(
        Knobs::from_fields(&[FieldValue::Bool(true)]).unwrap(),
        Knobs::default()
    );
    assert_eq!(
        OptionalDefaults::from_fields(&[]).unwrap(),
        OptionalDefaults::default()
    );
    assert_eq!(
        OptionalDefaults::from_fields(&[FieldValue::Null, FieldValue::Null]).unwrap(),
        OptionalDefaults {
            default_none: None,
            default_some: None,
        }
    );
    assert_eq!(
        OptionalDefaults::from_fields(&[
            FieldValue::Int(IntegerValue::Signed(7)),
            FieldValue::Int(IntegerValue::Unsigned(13)),
        ])
        .unwrap(),
        OptionalDefaults {
            default_none: Some(7),
            default_some: Some(13),
        }
    );
    assert_eq!(
        OptionalDefaults::from_fields(&[
            FieldValue::Bool(false),
            FieldValue::Str("wrong".to_string()),
        ])
        .unwrap(),
        OptionalDefaults::default()
    );

    let error = Knobs::from_fields(&[FieldValue::Int(IntegerValue::Signed(-1))]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "field `tile_size` value -1 cannot be represented as u32"
    );
    let error = Knobs::from_fields(&[
        FieldValue::Int(IntegerValue::Unsigned(128)),
        FieldValue::Float(f64::INFINITY),
    ])
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "field `threshold` value inf cannot be represented as f32"
    );
}

mod other {
    use crate::introspect::IntrospectEnum;

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
