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
// NaN and Infinity (lossy — serialized as null)
// ===========================================================================

#[test]
fn nan_serializes_as_null() {
    let scn = to_string(&f64::NAN).unwrap();
    assert_eq!(scn, "null\n");
    // Deserializes back as Option::None since null can't represent a float
    let v: Option<f64> = from_str(&scn).unwrap();
    assert_eq!(v, None);
}

#[test]
fn infinity_serializes_as_null() {
    let scn = to_string(&f64::INFINITY).unwrap();
    assert_eq!(scn, "null\n");
    let scn = to_string(&f64::NEG_INFINITY).unwrap();
    assert_eq!(scn, "null\n");
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
