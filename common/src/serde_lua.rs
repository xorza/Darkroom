use std::io::Write;

use mlua::{Lua, LuaSerdeExt, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum SerdeLuaError {
    #[error("Lua evaluation failed")]
    LuaEval(#[from] mlua::Error),
    #[error("Serialization failed")]
    SerdeJson(#[from] serde_json::Error),
    #[error("Invalid UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("IO error")]
    Io(#[from] std::io::Error),
}

pub type SerdeLuaResult<T> = Result<T, SerdeLuaError>;

pub fn from_slice<T: DeserializeOwned>(serialized: &[u8]) -> SerdeLuaResult<T> {
    from_str(std::str::from_utf8(serialized)?)
}

pub fn from_reader<T: DeserializeOwned, R: std::io::Read>(reader: &mut R) -> SerdeLuaResult<T> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    from_slice(&buf)
}

pub fn from_str<T: DeserializeOwned>(serialized: &str) -> SerdeLuaResult<T> {
    let lua = Lua::new();
    let trimmed = serialized.trim();
    let value = lua.load(trimmed).eval::<Value>()?;
    Ok(lua.from_value(value)?)
}

pub fn to_string<T: Serialize>(value: &T) -> SerdeLuaResult<String> {
    let mut out = Vec::new();
    to_writer(&mut out, value)?;
    Ok(String::from_utf8(out).expect("lua output should be valid utf-8"))
}

pub fn to_writer<W: Write, T: Serialize>(writer: &mut W, value: &T) -> SerdeLuaResult<()> {
    let json_value = serde_json::to_value(value)?;
    writer.write_all(b"return ")?;
    write_lua_value(&json_value, 0, writer)?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn write_lua_value<W: Write>(
    value: &serde_json::Value,
    indent: usize,
    out: &mut W,
) -> std::io::Result<()> {
    match value {
        serde_json::Value::Null => out.write_all(b"nil"),
        serde_json::Value::Bool(value) => out.write_all(if *value { b"true" } else { b"false" }),
        serde_json::Value::Number(value) => write!(out, "{}", value),
        serde_json::Value::String(value) => write_lua_string(value, out),
        serde_json::Value::Array(values) => write_lua_array(values, indent, out),
        serde_json::Value::Object(values) => write_lua_object(values, indent, out),
    }
}

fn write_lua_array<W: Write>(
    values: &[serde_json::Value],
    indent: usize,
    out: &mut W,
) -> std::io::Result<()> {
    if values.is_empty() {
        return out.write_all(b"{}");
    }

    out.write_all(b"{\n")?;
    let next_indent = indent + 1;
    for value in values {
        push_indent(next_indent, out)?;
        write_lua_value(value, next_indent, out)?;
        out.write_all(b",\n")?;
    }
    push_indent(indent, out)?;
    out.write_all(b"}")
}

fn write_lua_object<W: Write>(
    values: &serde_json::Map<String, serde_json::Value>,
    indent: usize,
    out: &mut W,
) -> std::io::Result<()> {
    if values.is_empty() {
        return out.write_all(b"{}");
    }

    out.write_all(b"{\n")?;
    let next_indent = indent + 1;
    for (key, value) in values {
        push_indent(next_indent, out)?;
        if is_lua_identifier(key) {
            out.write_all(key.as_bytes())?;
        } else {
            out.write_all(b"[")?;
            write_lua_string(key, out)?;
            out.write_all(b"]")?;
        }
        out.write_all(b" = ")?;
        write_lua_value(value, next_indent, out)?;
        out.write_all(b",\n")?;
    }
    push_indent(indent, out)?;
    out.write_all(b"}")
}

fn write_lua_string<W: Write>(value: &str, out: &mut W) -> std::io::Result<()> {
    out.write_all(b"\"")?;
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // ASCII escapes
        let esc: Option<&[u8]> = match b {
            b'\\' => Some(b"\\\\"),
            b'"' => Some(b"\\\""),
            b'\n' => Some(b"\\n"),
            b'\r' => Some(b"\\r"),
            b'\t' => Some(b"\\t"),
            _ => None,
        };
        if let Some(esc) = esc {
            out.write_all(&bytes[start..i])?;
            out.write_all(esc)?;
            i += 1;
            start = i;
            continue;
        }
        // Non-printable ASCII (control chars except already handled above)
        if b < 0x20 || b == 0x7F {
            out.write_all(&bytes[start..i])?;
            write!(out, "\\u{{{:x}}}", b)?;
            i += 1;
            start = i;
            continue;
        }
        // Non-ASCII: unicode-escape the full codepoint
        if b > 0x7F {
            let ch = value[i..].chars().next().unwrap();
            let ch_len = ch.len_utf8();
            out.write_all(&bytes[start..i])?;
            write!(out, "\\u{{{:x}}}", ch as u32)?;
            i += ch_len;
            start = i;
            continue;
        }
        i += 1;
    }
    out.write_all(&bytes[start..])?;
    out.write_all(b"\"")
}

const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

fn is_lua_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !matches!(bytes[0], b'a'..=b'z' | b'A'..=b'Z' | b'_') {
        return false;
    }
    if !bytes[1..]
        .iter()
        .all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'))
    {
        return false;
    }
    !LUA_KEYWORDS.contains(&value)
}

const INDENT_BUF: &[u8; 64] = b"                                                                ";

fn push_indent<W: Write>(indent: usize, out: &mut W) -> std::io::Result<()> {
    let mut bytes = indent * 2;
    while bytes > 0 {
        let n = bytes.min(INDENT_BUF.len());
        out.write_all(&INDENT_BUF[..n])?;
        bytes -= n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
        let lua = to_string(value).unwrap();
        let deserialized: T = from_str(&lua).unwrap();
        assert_eq!(*value, deserialized, "roundtrip failed for lua:\n{lua}");
    }

    #[test]
    fn primitives() {
        roundtrip(&true);
        roundtrip(&false);
        roundtrip(&42i32);
        roundtrip(&-7i64);
        roundtrip(&1.5f64);
        roundtrip(&0u64);
    }

    #[test]
    fn null_and_option() {
        roundtrip(&None::<i32>);
        roundtrip(&Some(42i32));
        roundtrip(&Some("hello".to_string()));
    }

    #[test]
    fn strings() {
        roundtrip(&"hello world".to_string());
        roundtrip(&"".to_string());
        roundtrip(&"line\nnewline".to_string());
        roundtrip(&"tab\there".to_string());
        roundtrip(&"quote\"end".to_string());
        roundtrip(&"back\\slash".to_string());
        roundtrip(&"carriage\rreturn".to_string());
    }

    #[test]
    fn sequences() {
        roundtrip(&vec![1i32, 2, 3]);
        roundtrip(&Vec::<i32>::new());
        roundtrip(&vec!["a".to_string(), "b".to_string()]);
        roundtrip(&vec![vec![1i32, 2], vec![3, 4]]);
    }

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

    #[test]
    fn vec_of_structs() {
        roundtrip(&vec![
            Simple {
                name: "a".to_string(),
                value: 1,
                flag: true,
            },
            Simple {
                name: "b".to_string(),
                value: 2,
                flag: false,
            },
        ]);
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
        let lua = b"return {name = \"test\", value = 42, flag = true}\n";
        let s: Simple = from_slice(lua).unwrap();
        assert_eq!(s.name, "test");
        assert_eq!(s.value, 42);
        assert!(s.flag);
    }

    #[test]
    fn is_lua_identifier_cases() {
        assert!(is_lua_identifier("hello"));
        assert!(is_lua_identifier("_private"));
        assert!(is_lua_identifier("camelCase"));
        assert!(is_lua_identifier("with_underscore"));
        assert!(is_lua_identifier("x1"));
        assert!(is_lua_identifier("_"));
        assert!(is_lua_identifier("A"));

        assert!(!is_lua_identifier(""));
        assert!(!is_lua_identifier("123"));
        assert!(!is_lua_identifier("has space"));
        assert!(!is_lua_identifier("has-dash"));
        assert!(!is_lua_identifier("1start"));
        assert!(!is_lua_identifier("with.dot"));

        // Lua keywords must not be bare identifiers
        assert!(!is_lua_identifier("true"));
        assert!(!is_lua_identifier("false"));
        assert!(!is_lua_identifier("nil"));
        assert!(!is_lua_identifier("return"));
        assert!(!is_lua_identifier("end"));
        assert!(!is_lua_identifier("function"));
        assert!(!is_lua_identifier("local"));
        assert!(!is_lua_identifier("if"));
        assert!(!is_lua_identifier("and"));
        assert!(!is_lua_identifier("or"));
        assert!(!is_lua_identifier("not"));
    }

    #[test]
    fn string_escaping_output() {
        let mut buf = Vec::new();
        write_lua_string("hello\nworld", &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), r#""hello\nworld""#);

        let mut buf = Vec::new();
        write_lua_string("tab\there", &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), r#""tab\there""#);

        let mut buf = Vec::new();
        write_lua_string(r#"quote"end"#, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), r#""quote\"end""#);

        let mut buf = Vec::new();
        write_lua_string("back\\slash", &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), r#""back\\slash""#);
    }

    #[test]
    fn unicode_escaped_in_lua() {
        let mut buf = Vec::new();
        write_lua_string("hello 世界", &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        // Non-ASCII should be unicode-escaped
        assert!(output.contains("\\u{"));
        assert!(output.starts_with('"'));
        assert!(output.ends_with('"'));

        // But it should roundtrip through Lua
        roundtrip(&"hello 世界".to_string());
    }

    #[test]
    fn control_chars_escaped() {
        let mut buf = Vec::new();
        write_lua_string("bell\x07here", &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\\u{7}"));
    }

    #[test]
    fn empty_collections_output() {
        let lua = to_string(&Vec::<i32>::new()).unwrap();
        assert!(lua.contains("{}"));

        let lua = to_string(&std::collections::BTreeMap::<String, i32>::new()).unwrap();
        assert!(lua.contains("{}"));
    }

    #[test]
    fn null_output() {
        let lua = to_string(&None::<i32>).unwrap();
        assert!(lua.contains("nil"));
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

    #[test]
    fn error_invalid_lua() {
        let result = from_str::<i32>("return {{{invalid");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SerdeLuaError::LuaEval(_)));
    }

    #[test]
    fn error_invalid_utf8() {
        let bytes: &[u8] = &[0xFF, 0xFE, 0x80];
        let result = from_slice::<i32>(bytes);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SerdeLuaError::Utf8(_)));
    }

    #[test]
    fn float_output() {
        // Negative float
        roundtrip(&-1.234f64);
        // Zero
        roundtrip(&0.0f64);
        // Large float
        roundtrip(&1e18f64);
        // Small float
        roundtrip(&1e-10f64);
    }

    #[test]
    fn output_format_structure() {
        let lua = to_string(&42i32).unwrap();
        assert!(lua.starts_with("return "), "should start with 'return '");
        assert!(lua.ends_with('\n'), "should end with newline");
        assert_eq!(lua, "return 42\n");
    }

    #[test]
    fn output_format_nested_indentation() {
        let lua = to_string(&vec![vec![1i32]]).unwrap();
        // Outer array items at 2 spaces, inner array items at 4 spaces
        assert!(lua.contains("  {\n    1,\n  },"));
    }

    #[test]
    fn del_char_escaped() {
        let mut buf = Vec::new();
        write_lua_string("before\x7Fafter", &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\\u{7f}"));
        // Roundtrip through Lua
        roundtrip(&"before\x7Fafter".to_string());
    }

    #[test]
    fn four_byte_unicode() {
        // Emoji (4-byte UTF-8)
        roundtrip(&"\u{1F600}".to_string());

        let mut buf = Vec::new();
        write_lua_string("\u{1F600}", &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\\u{1f600}"));
    }

    #[test]
    fn mixed_escapes_in_one_string() {
        let s = "line1\nline2\ttab\"quote\\back\x07bell\u{00e9}accent".to_string();
        roundtrip(&s);

        let mut buf = Vec::new();
        write_lua_string(&s, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\\n"));
        assert!(output.contains("\\t"));
        assert!(output.contains("\\\""));
        assert!(output.contains("\\\\"));
        assert!(output.contains("\\u{7}"));
        assert!(output.contains("\\u{e9}"));
    }

    #[test]
    fn deep_nesting_indentation() {
        // 35 levels deep — exceeds INDENT_BUF (64 bytes = 32 indent levels)
        // Build nested vecs manually via JSON
        let mut val = serde_json::Value::Number(serde_json::Number::from(1));
        for _ in 0..35 {
            val = serde_json::Value::Array(vec![val]);
        }

        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Wrapper {
            data: serde_json::Value,
        }
        let w = Wrapper { data: val };
        roundtrip(&w);
    }

    #[test]
    fn numeric_string_keys() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert("42".to_string(), "num".to_string());
        map.insert("true".to_string(), "bool-like".to_string());
        roundtrip(&map);

        // Verify they use bracket syntax, not bare identifiers
        let lua = to_string(&map).unwrap();
        assert!(lua.contains("[\"42\"]"));
        assert!(lua.contains("[\"true\"]"));
    }

    #[test]
    fn plain_ascii_string_no_escapes() {
        let mut buf = Vec::new();
        write_lua_string("hello world 123", &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\"hello world 123\"");
    }

    #[test]
    fn empty_string_output() {
        let mut buf = Vec::new();
        write_lua_string("", &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\"\"");
    }

    #[test]
    fn single_element_array_output() {
        let lua = to_string(&vec![99i32]).unwrap();
        assert_eq!(lua, "return {\n  99,\n}\n");
    }

    #[test]
    fn bool_output_exact() {
        assert_eq!(to_string(&true).unwrap(), "return true\n");
        assert_eq!(to_string(&false).unwrap(), "return false\n");
    }
}
