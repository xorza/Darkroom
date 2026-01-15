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
    writer.write_all(b"return ").unwrap();
    write_lua_value(&json_value, 0, writer);
    writer.write_all(b"\n").unwrap();
    Ok(())
}

fn write_lua_value<W: Write>(value: &serde_json::Value, indent: usize, out: &mut W) {
    match value {
        serde_json::Value::Null => out.write_all(b"nil").unwrap(),
        serde_json::Value::Bool(value) => {
            if *value {
                out.write_all(b"true").unwrap();
            } else {
                out.write_all(b"false").unwrap();
            }
        }
        serde_json::Value::Number(value) => write!(out, "{}", value).unwrap(),
        serde_json::Value::String(value) => write_lua_string(value, out),
        serde_json::Value::Array(values) => write_lua_array(values, indent, out),
        serde_json::Value::Object(values) => write_lua_object(values, indent, out),
    }
}

fn write_lua_array<W: Write>(values: &[serde_json::Value], indent: usize, out: &mut W) {
    if values.is_empty() {
        out.write_all(b"{}").unwrap();
        return;
    }

    out.write_all(b"{\n").unwrap();
    let next_indent = indent + 1;
    for (index, value) in values.iter().enumerate() {
        push_indent(next_indent, out);
        write_lua_value(value, next_indent, out);
        if index + 1 != values.len() {
            out.write_all(b",").unwrap();
        }
        out.write_all(b"\n").unwrap();
    }
    push_indent(indent, out);
    out.write_all(b"}").unwrap();
}

fn write_lua_object<W: Write>(
    values: &serde_json::Map<String, serde_json::Value>,
    indent: usize,
    out: &mut W,
) {
    if values.is_empty() {
        out.write_all(b"{}").unwrap();
        return;
    }

    out.write_all(b"{\n").unwrap();
    let next_indent = indent + 1;
    let len = values.len();
    for (index, (key, value)) in values.iter().enumerate() {
        push_indent(next_indent, out);
        if is_lua_identifier(key) {
            out.write_all(key.as_bytes()).unwrap();
        } else {
            out.write_all(b"[").unwrap();
            write_lua_string(key, out);
            out.write_all(b"]").unwrap();
        }
        out.write_all(b" = ").unwrap();
        write_lua_value(value, next_indent, out);
        if index + 1 != len {
            out.write_all(b",").unwrap();
        }
        out.write_all(b"\n").unwrap();
    }
    push_indent(indent, out);
    out.write_all(b"}").unwrap();
}

fn write_lua_string<W: Write>(value: &str, out: &mut W) {
    out.write_all(b"\"").unwrap();
    for ch in value.chars() {
        match ch {
            '\\' => out.write_all(b"\\\\").unwrap(),
            '"' => out.write_all(b"\\\"").unwrap(),
            '\n' => out.write_all(b"\\n").unwrap(),
            '\r' => out.write_all(b"\\r").unwrap(),
            '\t' => out.write_all(b"\\t").unwrap(),
            ch if ch.is_ascii_graphic() || ch == ' ' => {
                let mut buf = [0u8; 4];
                out.write_all(ch.encode_utf8(&mut buf).as_bytes()).unwrap();
            }
            ch => {
                write!(out, "\\u{{{:x}}}", ch as u32).unwrap();
            }
        }
    }
    out.write_all(b"\"").unwrap();
}

fn is_lua_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_lua_identifier_start(first) {
        return false;
    }
    chars.all(is_lua_identifier_continue)
}

fn is_lua_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_lua_identifier_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn push_indent<W: Write>(indent: usize, out: &mut W) {
    for _ in 0..indent {
        out.write_all(b"  ").unwrap();
    }
}
