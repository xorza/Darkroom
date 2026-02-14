use super::value::ScnValue;
use super::*;
use serde::{Deserialize, Serialize};

fn roundtrip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
    let scn = to_string(value).unwrap();
    let deserialized: T = from_str(&scn).unwrap();
    assert_eq!(*value, deserialized, "roundtrip failed for scn:\n{scn}");
}

// ===========================================================================
// Primitives
// ===========================================================================

#[test]
fn primitives() {
    roundtrip(&true);
    roundtrip(&false);
    roundtrip(&42i32);
    roundtrip(&-7i64);
    roundtrip(&0u64);
    roundtrip(&1.5f64);
    roundtrip(&-3.125f64);
}

#[test]
fn null_and_option() {
    roundtrip(&None::<i32>);
    roundtrip(&Some(42i32));
    roundtrip(&Some("hello".to_string()));
}

#[test]
fn integer_boundaries() {
    roundtrip(&i64::MIN);
    roundtrip(&i64::MAX);
    roundtrip(&u64::MAX);
    roundtrip(&0i64);
    roundtrip(&0u64);
}

#[test]
fn float_with_exponent() {
    roundtrip(&1e10f64);
    roundtrip(&1.5e-3f64);
}

#[test]
fn float_zero() {
    let scn = to_string(&0.0f64).unwrap();
    let v: f64 = from_str(&scn).unwrap();
    assert_eq!(v, 0.0);
}

// ===========================================================================
// Strings
// ===========================================================================

#[test]
fn strings() {
    roundtrip(&"hello world".to_string());
    roundtrip(&"".to_string());
    roundtrip(&"line\nnewline".to_string());
    roundtrip(&"tab\there".to_string());
    roundtrip(&"quote\"end".to_string());
    roundtrip(&"back\\slash".to_string());
    roundtrip(&"carriage\rreturn".to_string());
    roundtrip(&"null\0byte".to_string());
}

#[test]
fn unicode_strings() {
    // Non-ASCII characters are kept as-is (valid UTF-8)
    roundtrip(&"hello 世界".to_string());
}

#[test]
fn control_char_strings() {
    roundtrip(&"bell\x07here".to_string());
    roundtrip(&"del\x7Fhere".to_string());
}

#[test]
fn triple_quoted_string_parse() {
    let scn = r#"
"""
  hello
  world
  """
"#
    .trim();
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "hello\nworld");
}

// ===========================================================================
// Arrays
// ===========================================================================

#[test]
fn arrays() {
    roundtrip(&vec![1i32, 2, 3]);
    roundtrip(&Vec::<i32>::new());
    roundtrip(&vec!["a".to_string(), "b".to_string()]);
    roundtrip(&vec![vec![1i32, 2], vec![3, 4]]);
}

// ===========================================================================
// Structs
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Simple {
    name: String,
    value: i32,
    flag: bool,
}

#[test]
fn simple_struct() {
    roundtrip(&Simple {
        name: "test".to_string(),
        value: 42,
        flag: true,
    });
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Nested {
    inner: Simple,
    count: u64,
}

#[test]
fn nested_struct() {
    roundtrip(&Nested {
        inner: Simple {
            name: "inner".to_string(),
            value: -1,
            flag: false,
        },
        count: 100,
    });
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct WithOptional {
    required: String,
    optional: Option<i32>,
}

#[test]
fn struct_with_option() {
    roundtrip(&WithOptional {
        required: "hello".to_string(),
        optional: Some(5),
    });
    roundtrip(&WithOptional {
        required: "hello".to_string(),
        optional: None,
    });
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct WithVec {
    items: Vec<Simple>,
    tags: Vec<String>,
}

#[test]
fn struct_with_vecs() {
    roundtrip(&WithVec {
        items: vec![
            Simple {
                name: "first".to_string(),
                value: 1,
                flag: true,
            },
            Simple {
                name: "second".to_string(),
                value: 2,
                flag: false,
            },
        ],
        tags: vec!["tag1".to_string(), "tag2".to_string()],
    });
}

#[test]
fn empty_vecs() {
    roundtrip(&WithVec {
        items: vec![],
        tags: vec![],
    });
}

// ===========================================================================
// Enums (variants)
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum Color {
    Red,
    Green,
    Blue,
}

#[test]
fn unit_enum() {
    roundtrip(&Color::Red);
    roundtrip(&Color::Green);
    roundtrip(&Color::Blue);
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum Binding {
    None,
    Const(i32),
    Named { id: u32, label: String },
}

#[test]
fn enum_variants() {
    roundtrip(&Binding::None);
    roundtrip(&Binding::Const(42));
    roundtrip(&Binding::Named {
        id: 7,
        label: "test".to_string(),
    });
}

#[test]
fn vec_of_enums() {
    roundtrip(&vec![
        Binding::None,
        Binding::Const(10),
        Binding::Named {
            id: 1,
            label: "x".to_string(),
        },
    ]);
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum TupleVariant {
    Pair(i32, String),
}

#[test]
fn tuple_variant() {
    roundtrip(&TupleVariant::Pair(1, "hello".to_string()));
}

// ===========================================================================
// Newtype struct
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Id(u64);

#[test]
fn newtype_struct() {
    roundtrip(&Id(42));
    roundtrip(&Id(0));
}

// ===========================================================================
// Maps
// ===========================================================================

#[test]
fn hashmap() {
    use std::collections::BTreeMap;
    let mut map = BTreeMap::new();
    map.insert("key1".to_string(), 1i32);
    map.insert("key2".to_string(), 2);
    roundtrip(&map);
}

#[test]
fn non_identifier_keys() {
    use std::collections::BTreeMap;
    let mut map = BTreeMap::new();
    map.insert("has space".to_string(), 1i32);
    map.insert("has-dash".to_string(), 2);
    map.insert("123start".to_string(), 3);
    map.insert("".to_string(), 4);
    roundtrip(&map);
}

#[test]
fn numeric_string_keys() {
    use std::collections::BTreeMap;
    let mut map = BTreeMap::new();
    map.insert("42".to_string(), "num".to_string());
    map.insert("true".to_string(), "bool-like".to_string());
    roundtrip(&map);
}

// ===========================================================================
// Complex nested
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Complex {
    id: u32,
    name: String,
    bindings: Vec<Binding>,
    nested: Option<Box<Complex>>,
}

#[test]
fn complex_nested() {
    roundtrip(&Complex {
        id: 1,
        name: "root".to_string(),
        bindings: vec![Binding::Const(5), Binding::None],
        nested: Some(Box::new(Complex {
            id: 2,
            name: "child".to_string(),
            bindings: vec![],
            nested: None,
        })),
    });
}

// ===========================================================================
// Comments
// ===========================================================================

#[test]
fn comments_ignored() {
    let scn = r#"
{
  // This is a comment
  name: "hello" // inline comment
  value: 42
  flag: true
}
"#;
    let s: Simple = from_str(scn).unwrap();
    assert_eq!(s.name, "hello");
    assert_eq!(s.value, 42);
    assert!(s.flag);
}

// ===========================================================================
// Output format
// ===========================================================================

#[test]
fn output_format_simple() {
    let scn = to_string(&42i32).unwrap();
    assert_eq!(scn, "42\n");
}

#[test]
fn output_format_bool() {
    assert_eq!(to_string(&true).unwrap(), "true\n");
    assert_eq!(to_string(&false).unwrap(), "false\n");
}

#[test]
fn output_format_null() {
    assert_eq!(to_string(&None::<i32>).unwrap(), "null\n");
}

#[test]
fn output_format_unit_variant() {
    let scn = to_string(&Color::Red).unwrap();
    assert_eq!(scn, "Red\n");
}

#[test]
fn output_format_newtype_variant() {
    let scn = to_string(&Binding::Const(42)).unwrap();
    assert_eq!(scn, "Const 42\n");
}

#[test]
fn output_format_struct_variant() {
    let scn = to_string(&Binding::Named {
        id: 7,
        label: "test".to_string(),
    })
    .unwrap();
    assert!(scn.contains("Named {"));
    assert!(scn.contains("id: 7"));
    assert!(scn.contains("label: \"test\""));
}

#[test]
fn output_format_small_array_inline() {
    let scn = to_string(&vec![1i32, 2, 3]).unwrap();
    assert_eq!(scn, "[1, 2, 3]\n");
}

#[test]
fn output_format_empty_array() {
    let scn = to_string(&Vec::<i32>::new()).unwrap();
    assert_eq!(scn, "[]\n");
}

#[test]
fn output_format_empty_map() {
    use std::collections::BTreeMap;
    let scn = to_string(&BTreeMap::<String, i32>::new()).unwrap();
    assert_eq!(scn, "{}\n");
}

// ===========================================================================
// IO round trips
// ===========================================================================

#[test]
fn to_writer_from_reader() {
    let original = Simple {
        name: "test".to_string(),
        value: 99,
        flag: false,
    };
    let mut buf = Vec::new();
    to_writer(&mut buf, &original).unwrap();
    let deserialized: Simple = from_reader(&mut buf.as_slice()).unwrap();
    assert_eq!(original, deserialized);
}

#[test]
fn from_slice_works() {
    let scn = b"{ name: \"test\", value: 42, flag: true }\n";
    let s: Simple = from_slice(scn).unwrap();
    assert_eq!(s.name, "test");
    assert_eq!(s.value, 42);
    assert!(s.flag);
}

// ===========================================================================
// Error cases
// ===========================================================================

#[test]
fn error_unterminated_string() {
    let result = from_str::<String>("\"hello");
    assert!(result.is_err());
}

#[test]
fn error_unexpected_token() {
    let result = from_str::<i32>("}");
    assert!(result.is_err());
}

#[test]
fn error_trailing_garbage() {
    let result = from_str::<i32>("42 extra");
    assert!(result.is_err());
}

#[test]
fn error_trailing_dot_float() {
    // "1." must be rejected — require at least one digit after dot
    let result = from_str::<f64>("1.");
    assert!(result.is_err(), "trailing dot should be rejected");
    let result = from_str::<f64>("-3.");
    assert!(result.is_err(), "negative trailing dot should be rejected");
    // "1.0" is fine
    assert_eq!(from_str::<f64>("1.0").unwrap(), 1.0);
}

#[test]
fn error_leading_zeros() {
    let result = from_str::<i32>("007");
    assert!(result.is_err());
    let result = from_str::<i32>("-007");
    assert!(result.is_err());
    // 0 itself is fine
    assert_eq!(from_str::<i32>("0").unwrap(), 0);
    // 0.5 is fine
    assert_eq!(from_str::<f64>("0.5").unwrap(), 0.5);
}

// ===========================================================================
// Trailing commas
// ===========================================================================

#[test]
fn trailing_commas_array() {
    let scn = "[1, 2, 3,]";
    let v: Vec<i32> = from_str(scn).unwrap();
    assert_eq!(v, vec![1, 2, 3]);
}

#[test]
fn trailing_commas_map() {
    let scn = r#"{ name: "test", value: 42, flag: true, }"#;
    let s: Simple = from_str(scn).unwrap();
    assert_eq!(s.name, "test");
    assert_eq!(s.value, 42);
    assert!(s.flag);
}

// ===========================================================================
// Newline-separated (no commas)
// ===========================================================================

#[test]
fn newline_separated_array() {
    let scn = "[\n  1\n  2\n  3\n]";
    let v: Vec<i32> = from_str(scn).unwrap();
    assert_eq!(v, vec![1, 2, 3]);
}

#[test]
fn newline_separated_map() {
    let scn = "{\n  name: \"test\"\n  value: 42\n  flag: true\n}";
    let s: Simple = from_str(scn).unwrap();
    assert_eq!(s.name, "test");
    assert_eq!(s.value, 42);
    assert!(s.flag);
}

// ===========================================================================
// Variant as struct field
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct WithBinding {
    name: String,
    binding: Binding,
}

#[test]
fn variant_in_struct() {
    roundtrip(&WithBinding {
        name: "a".to_string(),
        binding: Binding::None,
    });
    roundtrip(&WithBinding {
        name: "b".to_string(),
        binding: Binding::Const(42),
    });
    roundtrip(&WithBinding {
        name: "c".to_string(),
        binding: Binding::Named {
            id: 1,
            label: "x".to_string(),
        },
    });
}

#[test]
fn vec_of_structs_with_variants() {
    roundtrip(&vec![
        WithBinding {
            name: "a".to_string(),
            binding: Binding::None,
        },
        WithBinding {
            name: "b".to_string(),
            binding: Binding::Const(10),
        },
        WithBinding {
            name: "c".to_string(),
            binding: Binding::Named {
                id: 2,
                label: "y".to_string(),
            },
        },
    ]);
}

// ===========================================================================
// Serde default fields
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct WithDefaults {
    name: String,
    #[serde(default)]
    count: i32,
    #[serde(default)]
    tags: Vec<String>,
}

#[test]
fn serde_defaults() {
    let scn = r#"{ name: "test", count: 0, tags: [] }"#;
    let v: WithDefaults = from_str(scn).unwrap();
    assert_eq!(v.name, "test");
    assert_eq!(v.count, 0);
    assert!(v.tags.is_empty());
    roundtrip(&v);
}

// ===========================================================================
// Float that must have decimal point
// ===========================================================================

#[test]
fn float_extreme_values() {
    // These previously panicked due to 32-byte stack buffer overflow in emit_float.
    // f64::MAX Display is ~309 chars, 5e-324 is ~326 chars. ryu handles all values safely.
    roundtrip(&f64::MAX);
    roundtrip(&f64::MIN);
    roundtrip(&f64::MIN_POSITIVE);
    roundtrip(&5e-324_f64); // smallest subnormal

    // Verify ryu produces scientific notation for extreme values
    let scn = to_string(&f64::MAX).unwrap();
    assert!(
        scn.contains('e') || scn.contains('E'),
        "f64::MAX should use scientific notation: {scn}"
    );
}

#[test]
fn float_keeps_decimal_point() {
    let scn = to_string(&1.0f64).unwrap();
    assert!(
        scn.trim().contains('.') || scn.trim().contains('e'),
        "float should contain '.' or 'e': {scn}"
    );
    let v: f64 = from_str(&scn).unwrap();
    assert_eq!(v, 1.0);
}

// ===========================================================================
// Variant nesting: variant containing variant
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum Outer {
    Inner(Binding),
}

#[test]
fn nested_variants() {
    roundtrip(&Outer::Inner(Binding::None));
    roundtrip(&Outer::Inner(Binding::Const(5)));
    roundtrip(&Outer::Inner(Binding::Named {
        id: 1,
        label: "x".to_string(),
    }));
}

// ===========================================================================
// NaN and Infinity literals
// ===========================================================================

#[test]
fn nan_roundtrip() {
    let scn = to_string(&f64::NAN).unwrap();
    assert_eq!(scn, "nan\n");
    let v: f64 = from_str(&scn).unwrap();
    assert!(v.is_nan());
}

#[test]
fn infinity_roundtrip() {
    let scn = to_string(&f64::INFINITY).unwrap();
    assert_eq!(scn, "inf\n");
    let v: f64 = from_str(&scn).unwrap();
    assert_eq!(v, f64::INFINITY);

    let scn = to_string(&f64::NEG_INFINITY).unwrap();
    assert_eq!(scn, "-inf\n");
    let v: f64 = from_str(&scn).unwrap();
    assert_eq!(v, f64::NEG_INFINITY);
}

#[test]
fn nan_inf_parse_bare() {
    // nan as bare keyword
    let val = super::parse::parse("nan").unwrap();
    assert!(matches!(val, ScnValue::Float(f) if f.is_nan()));

    // inf as bare keyword
    let val = super::parse::parse("inf").unwrap();
    assert!(matches!(val, ScnValue::Float(f) if f == f64::INFINITY));

    // -inf from read_number path
    let val = super::parse::parse("-inf").unwrap();
    assert!(matches!(val, ScnValue::Float(f) if f == f64::NEG_INFINITY));

    // -nan accepted (NaN has no meaningful sign)
    let val = super::parse::parse("-nan").unwrap();
    assert!(matches!(val, ScnValue::Float(f) if f.is_nan()));
}

#[test]
fn nan_inf_in_array() {
    // [nan, inf, -inf] → array of 3 floats
    let val = super::parse::parse("[nan, inf, -inf]").unwrap();
    let ScnValue::Array(items) = val else {
        panic!("expected array");
    };
    assert_eq!(items.len(), 3);
    assert!(matches!(items[0], ScnValue::Float(f) if f.is_nan()));
    assert!(matches!(items[1], ScnValue::Float(f) if f == f64::INFINITY));
    assert!(matches!(items[2], ScnValue::Float(f) if f == f64::NEG_INFINITY));
}

#[test]
fn nan_inf_not_partial_ident() {
    // "infinity" should parse as identifier, not "inf" + "inity"
    let val = super::parse::parse("infinity").unwrap();
    assert!(matches!(val, ScnValue::Variant(ref tag, None) if tag == "infinity"));

    // "nana" should parse as identifier
    let val = super::parse::parse("nana").unwrap();
    assert!(matches!(val, ScnValue::Variant(ref tag, None) if tag == "nana"));

    // "info" should parse as identifier
    let val = super::parse::parse("info").unwrap();
    assert!(matches!(val, ScnValue::Variant(ref tag, None) if tag == "info"));
}

// ===========================================================================
// Empty and whitespace-only input
// ===========================================================================

#[test]
fn error_empty_input() {
    let result = from_str::<i32>("");
    assert!(result.is_err());
}

#[test]
fn error_whitespace_only_input() {
    let result = from_str::<i32>("   \n\n  ");
    assert!(result.is_err());
}

#[test]
fn error_comment_only_input() {
    let result = from_str::<i32>("// just a comment\n");
    assert!(result.is_err());
}

// ===========================================================================
// Unicode escape roundtrip
// ===========================================================================

#[test]
fn unicode_escape_roundtrip() {
    // Emoji roundtrips through normal UTF-8 (not escaped)
    roundtrip(&"\u{1f600}".to_string());
    // Control chars use \u{hex} escape
    let scn = to_string(&"\x07".to_string()).unwrap();
    assert!(scn.contains("\\u{7}"));
    let v: String = from_str(&scn).unwrap();
    assert_eq!(v, "\x07");
}

#[test]
fn unicode_escape_parse() {
    let scn = r#""\u{1f600}""#;
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "\u{1f600}");
}

// ===========================================================================
// Triple-quoted string edge cases
// ===========================================================================

#[test]
fn triple_quoted_empty() {
    let scn = "\"\"\"\"\"\"";
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "");
}

#[test]
fn triple_quoted_single_line() {
    let scn = "\"\"\"\nhello\n\"\"\"";
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "hello");
}

#[test]
fn triple_quoted_preserves_internal_blank_lines() {
    let scn = "\"\"\"\na\n\nb\n\"\"\"";
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "a\n\nb");
}

// ===========================================================================
// Error: unterminated triple-quoted string
// ===========================================================================

#[test]
fn error_unterminated_triple_quoted() {
    let result = from_str::<String>("\"\"\"hello");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("unterminated"), "error was: {err}");
}

// ===========================================================================
// Error: unknown escape sequence
// ===========================================================================

#[test]
fn error_unknown_escape() {
    let result = from_str::<String>(r#""\q""#);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("unknown escape"), "error was: {err}");
}

// ===========================================================================
// Parse error positions
// ===========================================================================

#[test]
fn error_position_accuracy() {
    let input = "{\n  name: \"ok\"\n  value: }\n}";
    let result = from_str::<Simple>(input);
    assert!(result.is_err());
    let err = result.unwrap_err();
    match err {
        ScnError::Parse { line, .. } => {
            assert_eq!(line, 3, "error should be on line 3");
        }
        other => panic!("expected Parse error, got: {other}"),
    }
}

// ===========================================================================
// Variant with array payload
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum WithArray {
    Items(Vec<i32>),
}

#[test]
fn variant_with_array_payload() {
    roundtrip(&WithArray::Items(vec![1, 2, 3]));
    roundtrip(&WithArray::Items(vec![]));
}

// ===========================================================================
// Multiple consecutive commas rejected
// ===========================================================================

#[test]
fn error_multiple_consecutive_commas_array() {
    let result = from_str::<Vec<i32>>("[1,,2]");
    assert!(
        result.is_err(),
        "multiple consecutive commas should be rejected"
    );
}

#[test]
fn error_multiple_consecutive_commas_map() {
    let result = from_str::<Simple>(r#"{ name: "x",, value: 1, flag: true }"#);
    assert!(
        result.is_err(),
        "multiple consecutive commas should be rejected"
    );
}

// ===========================================================================
// Deeply nested structures
// ===========================================================================

#[test]
fn deeply_nested() {
    let mut val = Complex {
        id: 0,
        name: "leaf".to_string(),
        bindings: vec![],
        nested: None,
    };
    for i in 1..=20 {
        val = Complex {
            id: i,
            name: format!("level_{i}"),
            bindings: vec![Binding::Const(i as i32)],
            nested: Some(Box::new(val)),
        };
    }
    roundtrip(&val);
}

// ===========================================================================
// Spec coverage: reject yes/no/on/off as boolean aliases
// ===========================================================================

#[test]
fn reject_yaml_boolean_aliases() {
    // Spec says: "Only these two keywords [true/false]. No yes/no/on/off."
    // These should parse as variant identifiers, not booleans.
    // Deserializing as bool must fail.
    assert!(from_str::<bool>("yes").is_err());
    assert!(from_str::<bool>("no").is_err());
    assert!(from_str::<bool>("on").is_err());
    assert!(from_str::<bool>("off").is_err());

    // They parse as identifiers (variants), not as booleans
    let val = super::parse::parse("yes").unwrap();
    assert!(matches!(val, ScnValue::Variant(tag, None) if tag == "yes"));
}

// ===========================================================================
// Spec coverage: mixed-type arrays
// ===========================================================================

#[test]
fn mixed_type_array_parse() {
    // Spec example: ["mixed", 42, true, null]
    let val = super::parse::parse(r#"["mixed", 42, true, null]"#).unwrap();
    match val {
        ScnValue::Array(items) => {
            assert_eq!(items.len(), 4);
            assert_eq!(items[0], ScnValue::String("mixed".to_string()));
            assert_eq!(items[1], ScnValue::Int(42));
            assert_eq!(items[2], ScnValue::Bool(true));
            assert_eq!(items[3], ScnValue::Null);
        }
        other => panic!("expected array, got: {other:?}"),
    }
}

// ===========================================================================
// Spec coverage: variant with string payload
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum StringPayload {
    Const(String),
}

#[test]
fn variant_with_string_payload() {
    // Spec example: Const "hello"
    roundtrip(&StringPayload::Const("hello".to_string()));

    // Verify the serialized format
    let scn = to_string(&StringPayload::Const("hello".to_string())).unwrap();
    assert_eq!(scn, "Const \"hello\"\n");
}

// ===========================================================================
// Spec coverage: greedy parsing ambiguity
// ===========================================================================

#[test]
fn greedy_variant_parsing() {
    // Spec says: [None Const 10] parses as single nested variant
    let val = super::parse::parse("[None Const 10]").unwrap();
    let ScnValue::Array(items) = val else {
        panic!("expected array");
    };
    assert_eq!(items.len(), 1);
    // None consumed Const as payload, Const consumed 10 as payload
    let ScnValue::Variant(ref outer, Some(ref inner)) = items[0] else {
        panic!("expected variant, got: {:?}", items[0]);
    };
    assert_eq!(outer, "None");
    let ScnValue::Variant(ref tag, Some(ref payload)) = **inner else {
        panic!("expected inner variant, got: {inner:?}");
    };
    assert_eq!(tag, "Const");
    assert_eq!(**payload, ScnValue::Int(10));

    // With commas: [None, Const 10] → two separate variants
    let val = super::parse::parse("[None, Const 10]").unwrap();
    match val {
        ScnValue::Array(items) => {
            assert_eq!(items.len(), 2);
            assert!(matches!(&items[0], ScnValue::Variant(tag, None) if tag == "None"));
            assert!(matches!(&items[1], ScnValue::Variant(tag, Some(payload))
                if tag == "Const" && **payload == ScnValue::Int(10)));
        }
        other => panic!("expected array, got: {other:?}"),
    }
}

// ===========================================================================
// Spec coverage: integer vs float distinction
// ===========================================================================

#[test]
fn integer_vs_float_distinction() {
    // Spec: "Integer vs float is distinguished by the presence of . or e/E"
    let val = super::parse::parse("42").unwrap();
    assert!(matches!(val, ScnValue::Int(42)));

    let val = super::parse::parse("42.0").unwrap();
    assert!(matches!(val, ScnValue::Float(f) if f == 42.0));

    let val = super::parse::parse("42e0").unwrap();
    assert!(matches!(val, ScnValue::Float(f) if f == 42.0));

    let val = super::parse::parse("-7").unwrap();
    assert!(matches!(val, ScnValue::Int(-7)));

    let val = super::parse::parse("-7.0").unwrap();
    assert!(matches!(val, ScnValue::Float(f) if f == -7.0));
}

// ===========================================================================
// Spec coverage: triple-quoted with actual indent stripping
// ===========================================================================

#[test]
fn triple_quoted_indent_stripping() {
    // Closing """ indentation determines strip level
    let scn = "\"\"\"\n    line1\n    line2\n  \"\"\"";
    let v: String = from_str(scn).unwrap();
    // Closing """ has 2 spaces indent, so 2 spaces stripped from each line
    assert_eq!(v, "  line1\n  line2");

    // Closing """ at column 0: strips 0 spaces
    let scn = "\"\"\"\n  line1\n  line2\n\"\"\"";
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "  line1\n  line2");

    // Closing """ at same indent as content: strips all leading
    let scn = "\"\"\"\n  line1\n  line2\n  \"\"\"";
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "line1\nline2");
}

// ===========================================================================
// Spec coverage: comment at start of document
// ===========================================================================

#[test]
fn comment_at_start_of_document() {
    let scn = "// This is a leading comment\n42";
    let v: i32 = from_str(scn).unwrap();
    assert_eq!(v, 42);

    // Multiple leading comments
    let scn = "// comment 1\n// comment 2\n\"hello\"";
    let v: String = from_str(scn).unwrap();
    assert_eq!(v, "hello");
}

// ===========================================================================
// Spec coverage: underscore-prefixed identifiers as variants
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum WithUnderscore {
    _Private,
    __Internal(i32),
}

#[test]
fn underscore_prefixed_variant() {
    // Spec IDENT: [a-zA-Z_][a-zA-Z0-9_]*
    roundtrip(&WithUnderscore::_Private);
    roundtrip(&WithUnderscore::__Internal(42));

    // Verify it parses from text
    let val = super::parse::parse("_Private").unwrap();
    assert!(matches!(val, ScnValue::Variant(tag, None) if tag == "_Private"));

    let val = super::parse::parse("__Internal 42").unwrap();
    assert!(
        matches!(&val, ScnValue::Variant(tag, Some(payload)) if tag == "__Internal" && **payload == ScnValue::Int(42))
    );
}

// ===========================================================================
// Spec coverage: tab and CR as whitespace
// ===========================================================================

#[test]
fn tab_and_cr_as_whitespace() {
    // Spec: "Spaces, tabs, newlines, and carriage returns are whitespace"
    // Tabs between tokens
    let v: i32 = from_str("\t42\t").unwrap();
    assert_eq!(v, 42);

    // Tab-separated struct fields
    let scn = "{\tname:\t\"test\"\tvalue:\t42\tflag:\ttrue\t}";
    let s: Simple = from_str(scn).unwrap();
    assert_eq!(s.name, "test");
    assert_eq!(s.value, 42);
    assert!(s.flag);

    // CR+LF line endings
    let scn = "{\r\n  name: \"test\"\r\n  value: 42\r\n  flag: true\r\n}";
    let s: Simple = from_str(scn).unwrap();
    assert_eq!(s.name, "test");
    assert_eq!(s.value, 42);
    assert!(s.flag);
}

// ===========================================================================
// Spec coverage: full example (complex structure from spec)
// ===========================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum StaticValue {
    Int(i32),
    Float(f64),
    Str(String),
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum PortBinding {
    None,
    Bind { target_id: String, port_idx: u32 },
    Const(StaticValue),
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum Behavior {
    Once,
    Always,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Port {
    name: String,
    binding: PortBinding,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Event {
    name: String,
    subscribers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Node {
    id: String,
    func_id: String,
    name: String,
    behavior: Behavior,
    inputs: Vec<Port>,
    events: Vec<Event>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Graph {
    nodes: Vec<Node>,
}

#[test]
fn full_spec_example() {
    // Based on the SPEC.md full example
    let graph = Graph {
        nodes: vec![Node {
            id: "579ae1d6-10a3-4906-8948-135cb7d7508b".to_string(),
            func_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890".to_string(),
            name: "mult".to_string(),
            behavior: Behavior::Once,
            inputs: vec![
                Port {
                    name: "a".to_string(),
                    binding: PortBinding::Bind {
                        target_id: "999c4d37-e0eb-4856-be3f-ad2090c84d8c".to_string(),
                        port_idx: 0,
                    },
                },
                Port {
                    name: "b".to_string(),
                    binding: PortBinding::Const(StaticValue::Int(-7)),
                },
                Port {
                    name: "c".to_string(),
                    binding: PortBinding::None,
                },
            ],
            events: vec![Event {
                name: "on_complete".to_string(),
                subscribers: vec!["b88ab7e2-17b7-46cb-bc8e-b428bb45141e".to_string()],
            }],
        }],
    };

    // Roundtrip the complex structure
    roundtrip(&graph);

    // Also verify it can be parsed from hand-written SCN text
    let scn = r#"
// Scenarium graph
{
  nodes: [
    {
      id: "579ae1d6-10a3-4906-8948-135cb7d7508b",
      func_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
      name: "mult",
      behavior: Once,
      inputs: [
        {
          name: "a",
          binding: Bind {
            target_id: "999c4d37-e0eb-4856-be3f-ad2090c84d8c",
            port_idx: 0,
          },
        },
        {
          name: "b",
          binding: Const Int -7,
        },
        {
          name: "c",
          binding: None,
        },
      ],
      events: [
        {
          name: "on_complete",
          subscribers: [
            "b88ab7e2-17b7-46cb-bc8e-b428bb45141e",
          ],
        },
      ],
    },
  ],
}
"#;
    let parsed: Graph = from_str(scn).unwrap();
    assert_eq!(parsed, graph);
}

// ===========================================================================
// Hex/octal/binary integer literals
// ===========================================================================

#[test]
fn hex_integer_literals() {
    // 0xFF = 15*16 + 15 = 255
    let val = super::parse::parse("0xFF").unwrap();
    assert_eq!(val, ScnValue::Int(255));

    // Case-insensitive prefix and digits
    let val = super::parse::parse("0XFF").unwrap();
    assert_eq!(val, ScnValue::Int(255));
    let val = super::parse::parse("0xff").unwrap();
    assert_eq!(val, ScnValue::Int(255));
    let val = super::parse::parse("0xAb").unwrap();
    assert_eq!(val, ScnValue::Int(0xAB)); // 10*16 + 11 = 171

    // Negative hex: -0x10 = -16
    let val = super::parse::parse("-0x10").unwrap();
    assert_eq!(val, ScnValue::Int(-16));

    // Max u64: 0xFFFFFFFFFFFFFFFF
    let val = super::parse::parse("0xFFFFFFFFFFFFFFFF").unwrap();
    assert_eq!(val, ScnValue::Uint(u64::MAX));

    // Hex with underscores: 0xFF_FF = 65535
    let val = super::parse::parse("0xFF_FF").unwrap();
    assert_eq!(val, ScnValue::Int(65535));
}

#[test]
fn octal_integer_literals() {
    // 0o777 = 7*64 + 7*8 + 7 = 511
    let val = super::parse::parse("0o777").unwrap();
    assert_eq!(val, ScnValue::Int(511));

    // Case-insensitive prefix
    let val = super::parse::parse("0O10").unwrap();
    assert_eq!(val, ScnValue::Int(8)); // octal 10 = 8

    // Negative: -0o10 = -8
    let val = super::parse::parse("-0o10").unwrap();
    assert_eq!(val, ScnValue::Int(-8));
}

#[test]
fn binary_integer_literals() {
    // 0b1010 = 8 + 2 = 10
    let val = super::parse::parse("0b1010").unwrap();
    assert_eq!(val, ScnValue::Int(10));

    // Case-insensitive prefix
    let val = super::parse::parse("0B1111").unwrap();
    assert_eq!(val, ScnValue::Int(15)); // 8+4+2+1

    // Negative: -0b100 = -4
    let val = super::parse::parse("-0b100").unwrap();
    assert_eq!(val, ScnValue::Int(-4));
}

#[test]
fn prefixed_integer_roundtrip_emits_decimal() {
    // Parse hex, emit decimal
    let val = super::parse::parse("0xFF").unwrap();
    let mut buf = Vec::new();
    super::emit::emit_value(&mut buf, &val, 0, true).unwrap();
    assert_eq!(std::str::from_utf8(&buf).unwrap(), "255");

    // Parse binary, emit decimal
    let val = super::parse::parse("0b1010").unwrap();
    let mut buf = Vec::new();
    super::emit::emit_value(&mut buf, &val, 0, true).unwrap();
    assert_eq!(std::str::from_utf8(&buf).unwrap(), "10");
}

#[test]
fn error_prefixed_integer_no_digits() {
    assert!(from_str::<i32>("0x").is_err());
    assert!(from_str::<i32>("0o").is_err());
    assert!(from_str::<i32>("0b").is_err());
}

#[test]
fn error_prefixed_integer_invalid_digits() {
    // G is not a hex digit
    assert!(from_str::<i32>("0xGG").is_err());
    // 8 is not an octal digit
    assert!(from_str::<i32>("0o8").is_err());
    // 2 is not a binary digit
    assert!(from_str::<i32>("0b2").is_err());
}

#[test]
fn error_negative_hex_overflow() {
    // -0xFFFFFFFFFFFFFFFF overflows i64
    assert!(from_str::<i64>("-0xFFFFFFFFFFFFFFFF").is_err());
}

// ===========================================================================
// Underscore digit separators
// ===========================================================================

#[test]
fn underscore_integer_separators() {
    // 1_000_000 = 1000000
    let val = super::parse::parse("1_000_000").unwrap();
    assert_eq!(val, ScnValue::Int(1_000_000));

    // Single underscore: 1_0 = 10
    let val = super::parse::parse("1_0").unwrap();
    assert_eq!(val, ScnValue::Int(10));

    // Negative with underscores: -1_000 = -1000
    let val = super::parse::parse("-1_000").unwrap();
    assert_eq!(val, ScnValue::Int(-1000));
}

#[test]
fn underscore_float_separators() {
    // 1.23_45 = 1.2345
    let val = super::parse::parse("1.23_45").unwrap();
    assert_eq!(val, ScnValue::Float(1.2345));

    // 1_0e1_0 = 10e10 = 1e11
    let val = super::parse::parse("1_0e1_0").unwrap();
    assert_eq!(val, ScnValue::Float(10e10));

    // Underscore in integer part of float: 1_000.5
    let val = super::parse::parse("1_000.5").unwrap();
    assert_eq!(val, ScnValue::Float(1000.5));
}

#[test]
fn underscore_in_prefixed_integers() {
    // 0xFF_FF = 65535
    let val = super::parse::parse("0xFF_FF").unwrap();
    assert_eq!(val, ScnValue::Int(65535));

    // 0b1111_0000 = 240
    let val = super::parse::parse("0b1111_0000").unwrap();
    assert_eq!(val, ScnValue::Int(0b1111_0000)); // 240

    // 0o77_77 = 4095
    let val = super::parse::parse("0o77_77").unwrap();
    assert_eq!(val, ScnValue::Int(0o7777)); // 4095
}

#[test]
fn error_leading_underscore_in_number() {
    // _123 parses as identifier, not number — verify it fails as i32
    assert!(from_str::<i32>("_123").is_err());
}

#[test]
fn error_trailing_underscore_in_number() {
    assert!(from_str::<i32>("123_").is_err());
    assert!(from_str::<i32>("0xFF_").is_err());
}

#[test]
fn error_consecutive_underscores() {
    assert!(from_str::<i32>("1__2").is_err());
    assert!(from_str::<i32>("0x1__2").is_err());
}

#[test]
fn error_underscore_after_prefix() {
    // 0x_FF: underscore immediately after prefix
    assert!(from_str::<i32>("0x_FF").is_err());
    assert!(from_str::<i32>("0o_7").is_err());
    assert!(from_str::<i32>("0b_1").is_err());
}

#[test]
fn error_underscore_adjacent_to_dot() {
    // 1_.0: trailing underscore before dot
    assert!(from_str::<f64>("1_.0").is_err());
    // 1._0: underscore after dot (first fractional char must be digit)
    assert!(from_str::<f64>("1._0").is_err());
}

// ===========================================================================
// nan/inf as map keys
// ===========================================================================

#[test]
fn nan_inf_serde_roundtrip() {
    // nan through full serde path
    let scn = to_string(&f64::NAN).unwrap();
    assert_eq!(scn, "nan\n");
    let v: f64 = from_str("nan").unwrap();
    assert!(v.is_nan());

    // inf through serde
    assert_eq!(from_str::<f64>("inf").unwrap(), f64::INFINITY);
    assert_eq!(from_str::<f64>("-inf").unwrap(), f64::NEG_INFINITY);

    // nan/inf in struct field
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct WithFloat {
        x: f64,
        y: f64,
        z: f64,
    }
    let val = WithFloat {
        x: f64::NAN,
        y: f64::INFINITY,
        z: f64::NEG_INFINITY,
    };
    let scn = to_string(&val).unwrap();
    assert!(scn.contains("nan"), "NaN field: {scn}");
    assert!(scn.contains("inf"), "inf field: {scn}");
    assert!(scn.contains("-inf"), "-inf field: {scn}");
    let parsed: WithFloat = from_str(&scn).unwrap();
    assert!(parsed.x.is_nan());
    assert_eq!(parsed.y, f64::INFINITY);
    assert_eq!(parsed.z, f64::NEG_INFINITY);
}

#[test]
fn nan_inf_as_variant_payload() {
    // Variant consuming nan as payload: "Const nan" → Variant("Const", Float(NaN))
    let val = super::parse::parse("Const nan").unwrap();
    let ScnValue::Variant(ref tag, Some(ref payload)) = val else {
        panic!("expected variant, got {val:?}");
    };
    assert_eq!(tag, "Const");
    assert!(matches!(**payload, ScnValue::Float(f) if f.is_nan()));

    // Variant consuming inf
    let val = super::parse::parse("Const inf").unwrap();
    let ScnValue::Variant(_, Some(ref payload)) = val else {
        panic!("expected variant");
    };
    assert!(matches!(**payload, ScnValue::Float(f) if f == f64::INFINITY));
}

#[test]
fn hex_serde_deserialization() {
    // Full serde path, not just parse::parse
    assert_eq!(from_str::<i32>("0xFF").unwrap(), 255);
    assert_eq!(from_str::<i32>("-0x10").unwrap(), -16);
    assert_eq!(from_str::<u64>("0xFFFFFFFFFFFFFFFF").unwrap(), u64::MAX);
    assert_eq!(from_str::<i32>("0o77").unwrap(), 63); // 7*8 + 7
    assert_eq!(from_str::<i32>("0b1010").unwrap(), 10);
    assert_eq!(from_str::<i32>("1_000").unwrap(), 1000);
}

#[test]
fn hex_boundary_values() {
    // -0x8000000000000000 = -2^63 = i64::MIN
    let val = super::parse::parse("-0x8000000000000000").unwrap();
    assert_eq!(val, ScnValue::Int(i64::MIN));

    // 0x7FFFFFFFFFFFFFFF = i64::MAX → Int (not Uint)
    let val = super::parse::parse("0x7FFFFFFFFFFFFFFF").unwrap();
    assert_eq!(val, ScnValue::Int(i64::MAX));

    // 0x8000000000000000 = 2^63 → Uint (exceeds i64::MAX)
    let val = super::parse::parse("0x8000000000000000").unwrap();
    assert_eq!(val, ScnValue::Uint(0x8000000000000000));

    // -0x8000000000000001 → overflow error (exceeds i64::MIN magnitude)
    assert!(super::parse::parse("-0x8000000000000001").is_err());
}

#[test]
fn prefixed_zero_and_minimal() {
    // Zero in all bases
    let val = super::parse::parse("0x0").unwrap();
    assert_eq!(val, ScnValue::Int(0));
    let val = super::parse::parse("0o0").unwrap();
    assert_eq!(val, ScnValue::Int(0));
    let val = super::parse::parse("0b0").unwrap();
    assert_eq!(val, ScnValue::Int(0));

    // Negative zero → still Int(0) (i64 has no -0)
    let val = super::parse::parse("-0x0").unwrap();
    assert_eq!(val, ScnValue::Int(0));

    // Single digit
    let val = super::parse::parse("0x1").unwrap();
    assert_eq!(val, ScnValue::Int(1));
    let val = super::parse::parse("0o7").unwrap();
    assert_eq!(val, ScnValue::Int(7));
    let val = super::parse::parse("0b1").unwrap();
    assert_eq!(val, ScnValue::Int(1));

    // Leading zeros after prefix (valid in hex/oct/bin, unlike decimal)
    let val = super::parse::parse("0x0F").unwrap();
    assert_eq!(val, ScnValue::Int(15));
    let val = super::parse::parse("0o007").unwrap();
    assert_eq!(val, ScnValue::Int(7));
    let val = super::parse::parse("0b0001").unwrap();
    assert_eq!(val, ScnValue::Int(1));
}

#[test]
fn underscore_near_exponent() {
    // Underscore before 'e': 1_0e5 = 10e5 = 1_000_000
    let val = super::parse::parse("1_0e5").unwrap();
    assert_eq!(val, ScnValue::Float(10e5)); // 1_000_000.0

    // Underscore in exponent with sign: 1e+1_0 = 1e10
    let val = super::parse::parse("1e+1_0").unwrap();
    assert_eq!(val, ScnValue::Float(1e10));

    // Negative exponent with underscore: 1e-1_0 = 1e-10
    let val = super::parse::parse("1e-1_0").unwrap();
    assert_eq!(val, ScnValue::Float(1e-10));

    // Underscore before e is trailing for integer group → error
    assert!(super::parse::parse("1_e5").is_err());
}

#[test]
fn nan_inf_as_map_keys() {
    use std::collections::BTreeMap;
    let mut map = BTreeMap::new();
    map.insert("nan".to_string(), 1i32);
    map.insert("inf".to_string(), 2);
    roundtrip(&map);

    // Verify they're quoted in output (since nan/inf are keywords)
    let scn = to_string(&map).unwrap();
    assert!(scn.contains("\"inf\""), "inf key should be quoted: {scn}");
    assert!(scn.contains("\"nan\""), "nan key should be quoted: {scn}");
}

// ===========================================================================
// Recursion depth limit
// ===========================================================================

#[test]
fn deeply_nested_within_limit() {
    // 20 levels deep — well within the 128 limit
    // Already tested by `deeply_nested` test above, but verify explicitly
    let mut scn = String::new();
    for _ in 0..20 {
        scn.push('[');
    }
    scn.push('1');
    for _ in 0..20 {
        scn.push(']');
    }
    let v = super::parse::parse(&scn).unwrap();
    // Innermost is Int(1)
    let mut cur = &v;
    for _ in 0..20 {
        match cur {
            ScnValue::Array(items) => {
                assert_eq!(items.len(), 1);
                cur = &items[0];
            }
            other => panic!("expected array, got {other:?}"),
        }
    }
    assert_eq!(*cur, ScnValue::Int(1));
}

#[test]
fn error_exceeds_recursion_depth() {
    // 200 nested arrays — exceeds the 128 limit
    let mut scn = String::new();
    for _ in 0..200 {
        scn.push('[');
    }
    scn.push('1');
    for _ in 0..200 {
        scn.push(']');
    }
    let result = super::parse::parse(&scn);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nesting depth"),
        "expected depth error, got: {err}"
    );
}

#[test]
fn error_exceeds_recursion_depth_maps() {
    // Deeply nested maps: {a: {a: {a: ...}}}
    let mut scn = String::new();
    for _ in 0..200 {
        scn.push_str("{a: ");
    }
    scn.push('1');
    for _ in 0..200 {
        scn.push('}');
    }
    let result = super::parse::parse(&scn);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nesting depth"),
        "expected depth error, got: {err}"
    );
}

#[test]
fn error_exceeds_recursion_depth_variants() {
    // Deeply nested variants: A A A A ... 1
    let mut scn = String::new();
    for _ in 0..200 {
        scn.push_str("A ");
    }
    scn.push('1');
    let result = super::parse::parse(&scn);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nesting depth"),
        "expected depth error, got: {err}"
    );
}

// ===========================================================================
// UTF-8 string parsing (O(1) per character)
// ===========================================================================

#[test]
fn utf8_decode_2byte() {
    // decode_utf8_char 2-byte path: 0xC0..=0xDF leading byte
    // Smallest 2-byte: U+0080 = 0xC2 0x80
    let v: String = from_str("\"\\n\u{0080}\"").unwrap(); // escape forces slow path
    assert_eq!(v, "\n\u{0080}");
    // Largest 2-byte: U+07FF = 0xDF 0xBF
    let v: String = from_str("\"\\n\u{07FF}\"").unwrap();
    assert_eq!(v, "\n\u{07FF}");
    // Typical 2-byte: é = U+00E9 = 0xC3 0xA9
    let v: String = from_str("\"\\ncafé\"").unwrap();
    assert_eq!(v, "\ncafé");
}

#[test]
fn utf8_decode_3byte() {
    // decode_utf8_char 3-byte path: 0xE0..=0xEF leading byte
    // Smallest 3-byte: U+0800 = 0xE0 0xA0 0x80
    let v: String = from_str("\"\\n\u{0800}\"").unwrap();
    assert_eq!(v, "\n\u{0800}");
    // Largest 3-byte: U+FFFD (replacement character) = 0xEF 0xBF 0xBD
    let v: String = from_str("\"\\n\u{FFFD}\"").unwrap();
    assert_eq!(v, "\n\u{FFFD}");
    // Typical 3-byte: 世 = U+4E16
    let v: String = from_str("\"\\n世界\"").unwrap();
    assert_eq!(v, "\n世界");
}

#[test]
fn utf8_decode_4byte() {
    // decode_utf8_char 4-byte path: 0xF0..=0xF7 leading byte
    // Smallest 4-byte: U+10000 = 0xF0 0x90 0x80 0x80
    let v: String = from_str("\"\\n\u{10000}\"").unwrap();
    assert_eq!(v, "\n\u{10000}");
    // Largest valid: U+10FFFF = 0xF4 0x8F 0xBF 0xBF
    let v: String = from_str("\"\\n\u{10FFFF}\"").unwrap();
    assert_eq!(v, "\n\u{10FFFF}");
    // Typical 4-byte: 😀 = U+1F600
    let v: String = from_str("\"\\n😀\"").unwrap();
    assert_eq!(v, "\n😀");
}

#[test]
fn utf8_decode_mixed_boundaries() {
    // Mixed: ASCII + all three multibyte sizes, verifying decode_utf8_char
    // handles transitions between byte lengths correctly.
    // Forces slow path with leading escape.
    let input = "\"\\nA\u{00E9}\u{4E16}\u{1F600}B\u{07FF}\u{FFFD}\u{10FFFF}\"";
    let v: String = from_str(input).unwrap();
    assert_eq!(v, "\nA\u{00E9}\u{4E16}\u{1F600}B\u{07FF}\u{FFFD}\u{10FFFF}");
}

#[test]
fn utf8_string_with_escapes_and_multibyte() {
    // String that exercises the slow path: escape followed by multibyte
    let v: String = from_str("\"line1\\nCafé 世界\"").unwrap();
    assert_eq!(v, "line1\nCafé 世界");
}

#[test]
fn utf8_many_multibyte_chars() {
    // Previously O(n²) — verify it works correctly for longer strings.
    // 100 CJK characters: 你 = U+4F60 (3-byte UTF-8)
    let content = "你".repeat(100);
    let scn = format!("\"{}\"", content);
    // Force slow path by adding an escape at the start
    let scn_slow = format!("\"\\n{}\"", content);
    let v: String = from_str(&scn_slow).unwrap();
    assert_eq!(v, format!("\n{}", content));
    // Fast path
    let v: String = from_str(&scn).unwrap();
    assert_eq!(v, content);
}

// ===========================================================================
// IO error display
// ===========================================================================

#[test]
fn io_error_includes_message() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let scn_err = ScnError::Io(io_err);
    let msg = scn_err.to_string();
    assert!(
        msg.contains("file not found"),
        "IO error should include underlying message, got: {msg}"
    );
}
