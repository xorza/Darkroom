use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Simple {
    a: i32,
    b: String,
    c: bool,
    d: f64,
}

#[test]
fn round_trip_simple() {
    let v = Simple {
        a: 42,
        b: "hello".into(),
        c: true,
        d: 1.5,
    };
    let s = to_string(&v).unwrap();
    let back: Simple = from_str(&s).unwrap();
    assert_eq!(v, back);
}

#[test]
fn round_trip_float_integer_valued() {
    // Rhai distinguishes INT (i64) from FLOAT (f64). A float with integer
    // value must emit `.0` or it will parse as INT and from_dynamic will
    // refuse to hand it back as f64.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct F {
        x: f64,
    }
    let v = F { x: 1.0 };
    let s = to_string(&v).unwrap();
    assert!(s.contains("1.0"), "expected float literal, got: {s}");
    let back: F = from_str(&s).unwrap();
    assert_eq!(v, back);
}

#[test]
fn round_trip_option_none_is_unit() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct O {
        a: Option<i32>,
        b: Option<i32>,
    }
    let v = O {
        a: Some(7),
        b: None,
    };
    let s = to_string(&v).unwrap();
    let back: O = from_str(&s).unwrap();
    assert_eq!(v, back);
}

#[test]
fn round_trip_nested() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Inner {
        k: String,
    }
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Outer {
        items: Vec<Inner>,
        map: HashMap<String, i32>,
    }
    let mut map = HashMap::new();
    map.insert("alpha".into(), 1);
    map.insert("has space".into(), 2);
    let v = Outer {
        items: vec![
            Inner { k: "one".into() },
            Inner {
                k: "two\nlines".into(),
            },
        ],
        map,
    };
    let s = to_string(&v).unwrap();
    let back: Outer = from_str(&s).unwrap();
    assert_eq!(v, back);
}

#[test]
fn round_trip_enum_externally_tagged() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum E {
        Unit,
        Newtype(i32),
        Struct { x: i32, y: i32 },
    }
    for v in [E::Unit, E::Newtype(7), E::Struct { x: 1, y: 2 }] {
        let s = to_string(&v).unwrap();
        let back: E = from_str(&s).unwrap();
        assert_eq!(v, back, "serialized: {s}");
    }
}

#[test]
fn string_escapes() {
    let v = "a\"b\\c\nd\t\u{7}".to_string();
    let s = to_string(&v).unwrap();
    let back: String = from_str(&s).unwrap();
    assert_eq!(v, back);
}

#[test]
fn utf8_passthrough_not_escaped() {
    // Rhai accepts UTF-8 in string literals; non-ASCII should pass through
    // unescaped to keep files small and diff-friendly.
    let v = "text 🌍 日本語".to_string();
    let s = to_string(&v).unwrap();
    assert!(
        s.contains("text") && s.contains("🌍") && s.contains("日本語"),
        "expected UTF-8 passthrough, got: {s}"
    );
    let back: String = from_str(&s).unwrap();
    assert_eq!(v, back);
}

#[test]
fn keyword_key_is_quoted() {
    let mut m = std::collections::BTreeMap::new();
    m.insert("fn".to_string(), 1);
    m.insert("ok".to_string(), 2);
    let s = to_string(&m).unwrap();
    assert!(s.contains("\"fn\": 1"));
    assert!(s.contains("ok: 2"));
    let back: std::collections::BTreeMap<String, i32> = from_str(&s).unwrap();
    assert_eq!(m, back);
}

#[test]
fn eval_rejects_statements() {
    let err = from_str::<i32>("let x = 1; x");
    assert!(err.is_err());
}

#[test]
fn empty_containers() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct E {
        v: Vec<i32>,
        m: HashMap<String, i32>,
    }
    let v = E {
        v: vec![],
        m: HashMap::new(),
    };
    let s = to_string(&v).unwrap();
    let back: E = from_str(&s).unwrap();
    assert_eq!(v, back);
}
