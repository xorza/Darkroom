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
  name: "hello", // inline comment
  value: 42,
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
fn error_duplicate_map_keys() {
    // Duplicate bare key
    let result = from_str::<std::collections::HashMap<String, i32>>("{ a: 1, a: 2 }");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("duplicate"),
        "expected duplicate error, got: {err}"
    );

    // Duplicate quoted key
    let result = from_str::<std::collections::HashMap<String, i32>>(r#"{ "x": 1, "x": 2 }"#);
    assert!(result.is_err());

    // Duplicate with mixed bare/quoted (same string value)
    let result = from_str::<std::collections::HashMap<String, i32>>(r#"{ foo: 1, "foo": 2 }"#);
    assert!(result.is_err());

    // Non-duplicate is fine
    let v: std::collections::HashMap<String, i32> = from_str("{ a: 1, b: 2 }").unwrap();
    assert_eq!(v["a"], 1);
    assert_eq!(v["b"], 2);
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
// Missing commas rejected
// ===========================================================================

#[test]
fn error_missing_comma_array() {
    // Newline-separated without commas must be rejected
    let result = from_str::<Vec<i32>>("[\n  1\n  2\n  3\n]");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("expected ',' or ']'"),
        "expected comma error, got: {err}"
    );
}

#[test]
fn error_missing_comma_map() {
    let result = from_str::<Simple>("{\n  name: \"test\"\n  value: 42\n  flag: true\n}");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("expected ',' or '}'"),
        "expected comma error, got: {err}"
    );
}

#[test]
fn newline_separated_with_commas() {
    // Newlines with commas work fine
    let scn = "[\n  1,\n  2,\n  3\n]";
    let v: Vec<i32> = from_str(scn).unwrap();
    assert_eq!(v, vec![1, 2, 3]);

    let scn = "{\n  name: \"test\",\n  value: 42,\n  flag: true\n}";
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
    // Comma after first field, error on line 3 (unexpected } after value:)
    let input = "{\n  name: \"ok\",\n  value: }\n}";
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
    // [None Const 10] still parses as single nested variant (greedy consumes everything)
    let val = super::parse::parse("[None Const 10]").unwrap();
    let ScnValue::Array(items) = val else {
        panic!("expected array");
    };
    assert_eq!(items.len(), 1);
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

    // Greedy parsing still works for nested variants at top level
    // Const Int -7 → Variant("Const", Variant("Int", -7))
    let val = super::parse::parse("Const Int -7").unwrap();
    let ScnValue::Variant(ref outer, Some(ref inner)) = val else {
        panic!("expected variant, got: {val:?}");
    };
    assert_eq!(outer, "Const");
    let ScnValue::Variant(ref tag, Some(ref payload)) = **inner else {
        panic!("expected inner variant, got: {inner:?}");
    };
    assert_eq!(tag, "Int");
    assert_eq!(**payload, ScnValue::Int(-7));
}

#[test]
fn missing_comma_in_map_with_variant_caught() {
    // The key fix: { mode: Fast count: 10 } is now an error.
    // Without commas, "Fast" greedily consumes "count" as payload, then
    // the parser sees ":" which is not "," or "}" → error.
    let result = super::parse::parse("{ mode: Fast\n  count: 10 }");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("expected ',' or '}'"),
        "expected comma error, got: {err}"
    );

    // With commas it works correctly
    let val = super::parse::parse("{ mode: Fast, count: 10 }").unwrap();
    let ScnValue::Map(entries) = val else {
        panic!("expected map");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, "mode");
    assert!(matches!(&entries[0].1, ScnValue::Variant(tag, None) if tag == "Fast"));
    assert_eq!(entries[1].0, "count");
    assert_eq!(entries[1].1, ScnValue::Int(10));
}

#[test]
fn missing_comma_between_simple_values_caught() {
    // Even for non-variant values, commas are now required
    let result = from_str::<Vec<i32>>("[1 2 3]");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("expected ',' or ']'"),
        "expected comma error, got: {err}"
    );
}

#[test]
fn comma_required_single_item_no_comma_needed() {
    // Single-item collections don't need a comma
    assert_eq!(from_str::<Vec<i32>>("[42]").unwrap(), vec![42]);
    assert_eq!(
        from_str::<std::collections::HashMap<String, i32>>("{ x: 1 }")
            .unwrap()
            .get("x")
            .copied(),
        Some(1)
    );
    // Single variant in array
    let val = super::parse::parse("[None]").unwrap();
    assert!(matches!(
        val,
        ScnValue::Array(ref items) if items.len() == 1
            && matches!(&items[0], ScnValue::Variant(tag, None) if tag == "None")
    ));
}

#[test]
fn comma_required_between_various_value_types() {
    // Strings without comma
    let result = super::parse::parse(r#"["a" "b"]"#);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or ']'")
    );

    // Booleans without comma
    let result = super::parse::parse("[true false]");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or ']'")
    );

    // Nulls without comma
    let result = super::parse::parse("[null null]");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or ']'")
    );

    // Nested arrays without comma
    let result = super::parse::parse("[[1] [2]]");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or ']'")
    );

    // Maps without comma
    let result = super::parse::parse("[{a: 1} {b: 2}]");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or ']'")
    );

    // All with commas work
    assert!(super::parse::parse(r#"["a", "b"]"#).is_ok());
    assert!(super::parse::parse("[true, false]").is_ok());
    assert!(super::parse::parse("[null, null]").is_ok());
    assert!(super::parse::parse("[[1], [2]]").is_ok());
    assert!(super::parse::parse("[{a: 1}, {b: 2}]").is_ok());
}

#[test]
fn comma_required_partial_commas_caught() {
    // Some commas present, one missing: [1, 2 3]
    // Parser accepts 1 and 2 (comma between them), then 2's comma check
    // sees 3 (Int) which is not ',' or ']' → error
    let result = super::parse::parse("[1, 2 3]");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or ']'")
    );

    // Missing comma in map: { a: 1, b: 2 c: 3 }
    let result = super::parse::parse("{ a: 1, b: 2 c: 3 }");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or '}'")
    );
}

#[test]
fn comma_required_variant_then_nonvariant_in_array() {
    // Variant followed by non-variant without comma: [None 42]
    // "None" greedily consumes 42 as payload → Variant("None", Int(42))
    // Result: single-item array (greedy parsing wins)
    let val = super::parse::parse("[None 42]").unwrap();
    let ScnValue::Array(items) = val else {
        panic!("expected array");
    };
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ScnValue::Variant(tag, Some(payload))
            if tag == "None" && **payload == ScnValue::Int(42)
    ));

    // With comma: two separate items
    let val = super::parse::parse("[None, 42]").unwrap();
    let ScnValue::Array(items) = val else {
        panic!("expected array");
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], ScnValue::Variant(tag, None) if tag == "None"));
    assert_eq!(items[1], ScnValue::Int(42));
}

#[test]
fn comma_required_in_nested_collections() {
    // Missing comma in inner array
    let result = super::parse::parse("[[1 2], [3, 4]]");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or ']'")
    );

    // Missing comma in nested map
    let result = super::parse::parse("{ outer: { a: 1 b: 2 } }");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("expected ',' or '}'")
    );

    // Nested collections with all commas work
    assert!(super::parse::parse("[[1, 2], [3, 4]]").is_ok());
    assert!(super::parse::parse("{ outer: { a: 1, b: 2 } }").is_ok());
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

    // Tab-separated struct fields (with commas)
    let scn = "{\tname:\t\"test\",\tvalue:\t42,\tflag:\ttrue\t}";
    let s: Simple = from_str(scn).unwrap();
    assert_eq!(s.name, "test");
    assert_eq!(s.value, 42);
    assert!(s.flag);

    // CR+LF line endings (with commas)
    let scn = "{\r\n  name: \"test\",\r\n  value: 42,\r\n  flag: true\r\n}";
    let s: Simple = from_str(scn).unwrap();
    assert_eq!(s.name, "test");
    assert_eq!(s.value, 42);
    assert!(s.flag);

    // Tab-separated without commas is now rejected
    let result = from_str::<Simple>("{\tname:\t\"test\"\tvalue:\t42\tflag:\ttrue\t}");
    assert!(result.is_err());
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

    // Max u64: 0xFFFFFFFFFFFFFFFF — fits in i128, so Int
    let val = super::parse::parse("0xFFFFFFFFFFFFFFFF").unwrap();
    assert_eq!(val, ScnValue::Int(u64::MAX as i128));

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
    // -0x8000000000000000 = -2^63 = i64::MIN — fits in i128
    let val = super::parse::parse("-0x8000000000000000").unwrap();
    assert_eq!(val, ScnValue::Int(i64::MIN as i128));

    // 0x7FFFFFFFFFFFFFFF = i64::MAX → Int
    let val = super::parse::parse("0x7FFFFFFFFFFFFFFF").unwrap();
    assert_eq!(val, ScnValue::Int(i64::MAX as i128));

    // 0x8000000000000000 = 2^63 → Int (fits in i128)
    let val = super::parse::parse("0x8000000000000000").unwrap();
    assert_eq!(val, ScnValue::Int(0x8000000000000000_i128));

    // -0x8000000000000001 → fits in i128 (no longer overflow)
    let val = super::parse::parse("-0x8000000000000001").unwrap();
    assert_eq!(val, ScnValue::Int(-0x8000000000000001_i128));
}

#[test]
fn i128_boundary_values() {
    // Values beyond i64 range — parse as Int(i128)
    // i64::MAX + 1 = 9223372036854775808
    let val = super::parse::parse("9223372036854775808").unwrap();
    assert_eq!(val, ScnValue::Int(i64::MAX as i128 + 1));

    // i64::MIN - 1 = -9223372036854775809
    let val = super::parse::parse("-9223372036854775809").unwrap();
    assert_eq!(val, ScnValue::Int(i64::MIN as i128 - 1));

    // u64::MAX = 18446744073709551615 — fits in i128, so Int
    let val = super::parse::parse("18446744073709551615").unwrap();
    assert_eq!(val, ScnValue::Int(u64::MAX as i128));

    // u64::MAX + 1 = 18446744073709551616 — still fits in i128
    let val = super::parse::parse("18446744073709551616").unwrap();
    assert_eq!(val, ScnValue::Int(u64::MAX as i128 + 1));

    // i128::MAX = 170141183460469231731687303715884105727 — Int
    let val = super::parse::parse("170141183460469231731687303715884105727").unwrap();
    assert_eq!(val, ScnValue::Int(i128::MAX));

    // i128::MAX + 1 = 170141183460469231731687303715884105728 — Uint (exceeds i128::MAX)
    let val = super::parse::parse("170141183460469231731687303715884105728").unwrap();
    assert_eq!(val, ScnValue::Uint(i128::MAX as u128 + 1));

    // i128::MIN = -170141183460469231731687303715884105728
    let val = super::parse::parse("-170141183460469231731687303715884105728").unwrap();
    assert_eq!(val, ScnValue::Int(i128::MIN));

    // Beyond i128::MIN → error
    assert!(super::parse::parse("-170141183460469231731687303715884105729").is_err());

    // u128::MAX = 340282366920938463463374607431768211455 — Uint
    let val = super::parse::parse("340282366920938463463374607431768211455").unwrap();
    assert_eq!(val, ScnValue::Uint(u128::MAX));

    // Beyond u128::MAX → error
    assert!(super::parse::parse("340282366920938463463374607431768211456").is_err());
}

#[test]
fn i128_serde_roundtrip() {
    // i128 roundtrip through serde
    let big_pos: i128 = i64::MAX as i128 + 1000;
    let scn = to_string(&big_pos).unwrap();
    assert_eq!(from_str::<i128>(&scn).unwrap(), big_pos);

    let big_neg: i128 = i64::MIN as i128 - 1000;
    let scn = to_string(&big_neg).unwrap();
    assert_eq!(from_str::<i128>(&scn).unwrap(), big_neg);

    // u128 roundtrip
    let big_u: u128 = u64::MAX as u128 + 1000;
    let scn = to_string(&big_u).unwrap();
    assert_eq!(from_str::<u128>(&scn).unwrap(), big_u);

    // i128::MAX and i128::MIN roundtrip
    let scn = to_string(&i128::MAX).unwrap();
    assert_eq!(from_str::<i128>(&scn).unwrap(), i128::MAX);

    let scn = to_string(&i128::MIN).unwrap();
    assert_eq!(from_str::<i128>(&scn).unwrap(), i128::MIN);
}

#[test]
fn i128_serde_deserialization_paths() {
    // i128 struct field with value beyond i64 range
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct BigInts {
        signed: i128,
        unsigned: u128,
    }

    let input = "{ signed: 9223372036854775808, unsigned: 18446744073709551616 }";
    let val: BigInts = from_str(input).unwrap();
    // signed: i64::MAX + 1 = 9223372036854775808
    assert_eq!(val.signed, i64::MAX as i128 + 1);
    // unsigned: u64::MAX + 1 = 18446744073709551616
    assert_eq!(val.unsigned, u64::MAX as u128 + 1);

    // Positive Int(i128) fitting in u64 — tests deserialize_u64 visit_u64 path
    // Parser returns Int(18446744073709551615) for u64::MAX (fits in i128)
    // deserialize_u64 must route this through visit_u64
    assert_eq!(from_str::<u64>("18446744073709551615").unwrap(), u64::MAX);

    // Negative i128 through serde
    assert_eq!(
        from_str::<i128>("-9223372036854775809").unwrap(),
        i64::MIN as i128 - 1
    );

    // Overflow: i128 value too large for i64 target
    assert!(from_str::<i64>("9223372036854775808").is_err());

    // Overflow: u128 value too large for u64 target
    assert!(from_str::<u64>("18446744073709551616").is_err());

    // Large int into f64 — value beyond i64 but deserialize_f64 handles it
    assert_eq!(
        from_str::<f64>("9223372036854775808").unwrap(),
        9223372036854775808.0_f64
    );
}

#[test]
fn i128_hex_values() {
    // Hex values beyond u64 range
    // 0x1_0000_0000_0000_0000 = 2^64 = u64::MAX + 1
    let val = super::parse::parse("0x10000000000000000").unwrap();
    assert_eq!(val, ScnValue::Int(u64::MAX as i128 + 1));

    // Max i128 in hex: 0x7FFF...F (32 F's)
    let val = super::parse::parse("0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").unwrap();
    assert_eq!(val, ScnValue::Int(i128::MAX));

    // i128::MAX + 1 in hex → Uint
    let val = super::parse::parse("0x80000000000000000000000000000000").unwrap();
    assert_eq!(val, ScnValue::Uint(i128::MAX as u128 + 1));

    // u128::MAX in hex: 0xFFFF...F (32 F's) → Uint
    let val = super::parse::parse("0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").unwrap();
    assert_eq!(val, ScnValue::Uint(u128::MAX));

    // Beyond u128::MAX in hex → error
    assert!(super::parse::parse("0x1FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").is_err());
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
// Public API: to_value, from_value, ScnValue Display
// ===========================================================================

#[test]
fn to_value_struct() {
    #[derive(Serialize)]
    struct Point {
        x: i32,
        y: i32,
    }
    let v = to_value(&Point { x: 10, y: 20 }).unwrap();
    // Struct serializes as Map with field names as keys
    assert_eq!(
        v,
        ScnValue::Map(vec![
            ("x".to_string(), ScnValue::Int(10)),
            ("y".to_string(), ScnValue::Int(20)),
        ])
    );
}

#[test]
fn from_value_struct() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct Point {
        x: i32,
        y: i32,
    }
    let v = ScnValue::Map(vec![
        ("x".to_string(), ScnValue::Int(10)),
        ("y".to_string(), ScnValue::Int(20)),
    ]);
    let p: Point = from_value(v).unwrap();
    assert_eq!(p, Point { x: 10, y: 20 });
}

#[test]
fn to_from_value_roundtrip() {
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Config {
        name: String,
        count: u32,
        enabled: bool,
    }
    let original = Config {
        name: "test".to_string(),
        count: 42,
        enabled: true,
    };
    let v = to_value(&original).unwrap();
    let restored: Config = from_value(v).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn to_value_enum_variant() {
    #[derive(Serialize)]
    enum Action {
        Stop,
        Move { x: f64, y: f64 },
    }
    let v = to_value(&Action::Stop).unwrap();
    assert_eq!(v, ScnValue::Variant("Stop".to_string(), None));

    let v = to_value(&Action::Move { x: 1.0, y: 2.0 }).unwrap();
    assert_eq!(
        v,
        ScnValue::Variant(
            "Move".to_string(),
            Some(Box::new(ScnValue::Map(vec![
                ("x".to_string(), ScnValue::Float(1.0)),
                ("y".to_string(), ScnValue::Float(2.0)),
            ])))
        )
    );
}

#[test]
fn scn_value_display() {
    // Null
    assert_eq!(ScnValue::Null.to_string(), "null");
    // Bool
    assert_eq!(ScnValue::Bool(true).to_string(), "true");
    // Int
    assert_eq!(ScnValue::Int(-42).to_string(), "-42");
    // Uint
    assert_eq!(ScnValue::Uint(255).to_string(), "255");
    // Float
    assert_eq!(ScnValue::Float(1.5).to_string(), "1.5");
    // String (with escaping)
    assert_eq!(
        ScnValue::String("hello\nworld".to_string()).to_string(),
        r#""hello\nworld""#
    );
    // Array (inline for small simple values)
    assert_eq!(
        ScnValue::Array(vec![ScnValue::Int(1), ScnValue::Int(2)]).to_string(),
        "[1, 2]"
    );
    // Variant
    assert_eq!(
        ScnValue::Variant("None".to_string(), None).to_string(),
        "None"
    );
    assert_eq!(
        ScnValue::Variant("Some".to_string(), Some(Box::new(ScnValue::Int(42)))).to_string(),
        "Some 42"
    );
}

#[test]
fn scn_value_display_map() {
    // Map emits multiline with indentation
    let v = ScnValue::Map(vec![
        ("a".to_string(), ScnValue::Int(1)),
        ("b".to_string(), ScnValue::Int(2)),
    ]);
    let s = v.to_string();
    // Should contain key: value pairs
    assert!(s.contains("a: 1"), "got: {s}");
    assert!(s.contains("b: 2"), "got: {s}");
    assert!(s.starts_with('{'), "got: {s}");
    assert!(s.ends_with('}'), "got: {s}");
}

#[test]
fn scn_value_display_special_floats() {
    assert_eq!(ScnValue::Float(f64::NAN).to_string(), "nan");
    assert_eq!(ScnValue::Float(f64::INFINITY).to_string(), "inf");
    assert_eq!(ScnValue::Float(f64::NEG_INFINITY).to_string(), "-inf");
}

#[test]
fn from_value_enum_variant() {
    #[derive(Debug, Deserialize, PartialEq)]
    enum Color {
        Red,
        Rgb { r: u8, g: u8, b: u8 },
    }
    // Unit variant
    let v = ScnValue::Variant("Red".to_string(), None);
    let c: Color = from_value(v).unwrap();
    assert_eq!(c, Color::Red);
    // Struct variant
    let v = ScnValue::Variant(
        "Rgb".to_string(),
        Some(Box::new(ScnValue::Map(vec![
            ("r".to_string(), ScnValue::Uint(255)),
            ("g".to_string(), ScnValue::Uint(128)),
            ("b".to_string(), ScnValue::Uint(0)),
        ]))),
    );
    let c: Color = from_value(v).unwrap();
    assert_eq!(
        c,
        Color::Rgb {
            r: 255,
            g: 128,
            b: 0
        }
    );
}

#[test]
fn from_value_type_mismatch() {
    // Trying to deserialize an Int as a String should fail
    let v = ScnValue::Int(42);
    let result: std::result::Result<String, _> = from_value(v);
    assert!(result.is_err());
    // Trying to deserialize a String as a struct should fail
    let v = ScnValue::String("not a struct".to_string());
    let result: std::result::Result<Vec<i32>, _> = from_value(v);
    assert!(result.is_err());
}

#[test]
fn display_output_is_parseable() {
    // Display output of various ScnValue types should parse back correctly.
    // Note: Uint→text→parse produces Int (parser uses Int for positive values that fit i64),
    // so we test Uint separately below.
    let values = vec![
        ScnValue::Null,
        ScnValue::Bool(false),
        ScnValue::Int(-42),
        ScnValue::Int(999),
        ScnValue::Float(2.5),
        ScnValue::String("hello \"world\"".to_string()),
        ScnValue::Array(vec![ScnValue::Int(1), ScnValue::Int(2), ScnValue::Int(3)]),
        ScnValue::Variant("Stop".to_string(), None),
        ScnValue::Variant("Go".to_string(), Some(Box::new(ScnValue::Int(5)))),
    ];
    for original in &values {
        let displayed = original.to_string();
        let parsed = super::parse::parse(&displayed).unwrap();
        assert_eq!(
            *original, parsed,
            "Display→parse roundtrip failed for {original:?}\ndisplayed: {displayed}"
        );
    }

    // Uint that fits in i128 parses back as Int (correct — no Uint literal in grammar)
    let displayed = ScnValue::Uint(999).to_string();
    assert_eq!(displayed, "999");
    assert_eq!(super::parse::parse(&displayed).unwrap(), ScnValue::Int(999));

    // Uint(u64::MAX) fits in i128, so parses back as Int
    let big = u64::MAX as u128;
    let displayed = ScnValue::Uint(big).to_string();
    assert_eq!(
        super::parse::parse(&displayed).unwrap(),
        ScnValue::Int(big as i128)
    );
}

#[test]
fn display_map_is_parseable() {
    let v = ScnValue::Map(vec![
        ("name".to_string(), ScnValue::String("Alice".to_string())),
        ("age".to_string(), ScnValue::Int(30)),
    ]);
    let displayed = v.to_string();
    let parsed = super::parse::parse(&displayed).unwrap();
    assert_eq!(v, parsed);
}

#[test]
fn display_nested_variant_with_map() {
    // Variant with struct payload — multiline output
    let v = ScnValue::Variant(
        "Move".to_string(),
        Some(Box::new(ScnValue::Map(vec![
            ("x".to_string(), ScnValue::Float(1.0)),
            ("y".to_string(), ScnValue::Float(2.0)),
        ]))),
    );
    let displayed = v.to_string();
    let parsed = super::parse::parse(&displayed).unwrap();
    assert_eq!(v, parsed);
}

#[test]
fn display_large_array_multiline() {
    // >8 items with non-simple values triggers multiline formatting
    let items: Vec<ScnValue> = (0..10)
        .map(|i| ScnValue::Array(vec![ScnValue::Int(i), ScnValue::Int(i * 2)]))
        .collect();
    let v = ScnValue::Array(items);
    let displayed = v.to_string();
    // Should be multiline (contains newlines)
    assert!(
        displayed.contains('\n'),
        "expected multiline, got: {displayed}"
    );
    let parsed = super::parse::parse(&displayed).unwrap();
    assert_eq!(v, parsed);
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

// ===========================================================================
// Keyword variant names rejected at serialization time
// ===========================================================================

#[test]
fn error_keyword_variant_name_unit() {
    // Unit variants renamed to keywords must be rejected
    #[derive(Serialize)]
    enum Bad {
        #[serde(rename = "true")]
        Yes,
    }
    let result = to_string(&Bad::Yes);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not a valid SCN identifier"),
        "expected identifier error, got: {err}"
    );
}

#[test]
fn error_keyword_variant_name_newtype() {
    // Newtype variants renamed to keywords must be rejected
    #[derive(Serialize)]
    enum Bad {
        #[serde(rename = "null")]
        Val(i32),
    }
    let result = to_string(&Bad::Val(42));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not a valid SCN identifier"),
        "expected identifier error, got: {err}"
    );
}

#[test]
fn error_keyword_variant_name_struct() {
    // Struct variants renamed to keywords must be rejected
    #[derive(Serialize)]
    enum Bad {
        #[serde(rename = "nan")]
        Data { x: i32 },
    }
    let result = to_string(&Bad::Data { x: 1 });
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not a valid SCN identifier"),
        "expected identifier error, got: {err}"
    );
}

#[test]
fn error_keyword_variant_name_tuple() {
    // Tuple variants renamed to keywords must be rejected
    #[derive(Serialize)]
    enum Bad {
        #[serde(rename = "false")]
        Pair(i32, String),
    }
    let result = to_string(&Bad::Pair(1, "hi".to_string()));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not a valid SCN identifier"),
        "expected identifier error, got: {err}"
    );
}

#[test]
fn error_keyword_variant_name_all_keywords() {
    // All SCN keywords should be rejected: true, false, null, nan, inf
    #[derive(Serialize)]
    enum Keywords {
        #[serde(rename = "true")]
        A,
        #[serde(rename = "false")]
        B,
        #[serde(rename = "null")]
        C,
        #[serde(rename = "nan")]
        D,
        #[serde(rename = "inf")]
        E,
    }
    assert!(to_string(&Keywords::A).is_err());
    assert!(to_string(&Keywords::B).is_err());
    assert!(to_string(&Keywords::C).is_err());
    assert!(to_string(&Keywords::D).is_err());
    assert!(to_string(&Keywords::E).is_err());
}

#[test]
fn valid_variant_names_accepted() {
    // Normal variant names must still work
    #[derive(Serialize)]
    enum Good {
        Normal,
        #[serde(rename = "CamelCase")]
        Renamed,
        #[serde(rename = "_private")]
        UnderscoreStart,
    }
    assert!(to_string(&Good::Normal).is_ok());
    assert!(to_string(&Good::Renamed).is_ok());
    assert!(to_string(&Good::UnderscoreStart).is_ok());
}

