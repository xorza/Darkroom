use mlua::{Lua, LuaSerdeExt, Value};
use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum SerdeLuaError {
    #[error("Lua evaluation failed")]
    LuaEval(#[from] mlua::Error),
    #[error("Serialization failed")]
    SerdeJson(#[from] serde_json::Error),
}

pub type SerdeLuaResult<T> = Result<T, SerdeLuaError>;

pub fn from_str<T: DeserializeOwned>(serialized: &str) -> SerdeLuaResult<T> {
    let lua = Lua::new();
    let trimmed = serialized.trim();
    let value = lua.load(trimmed).eval::<Value>()?;
    Ok(lua.from_value(value)?)
}

pub fn to_string<T: Serialize>(value: &T) -> SerdeLuaResult<String> {
    let json_value = serde_json::to_value(value)?;
    let mut out = String::new();
    out.push_str("return ");
    write_lua_value(&json_value, 0, &mut out);
    out.push('\n');
    Ok(out)
}

fn write_lua_value(value: &serde_json::Value, indent: usize, out: &mut String) {
    match value {
        serde_json::Value::Null => out.push_str("nil"),
        serde_json::Value::Bool(value) => {
            if *value {
                out.push_str("true");
            } else {
                out.push_str("false");
            }
        }
        serde_json::Value::Number(value) => out.push_str(&value.to_string()),
        serde_json::Value::String(value) => write_lua_string(value, out),
        serde_json::Value::Array(values) => write_lua_array(values, indent, out),
        serde_json::Value::Object(values) => write_lua_object(values, indent, out),
    }
}

fn write_lua_array(values: &[serde_json::Value], indent: usize, out: &mut String) {
    if values.is_empty() {
        out.push_str("{}");
        return;
    }

    out.push_str("{\n");
    let next_indent = indent + 1;
    for (index, value) in values.iter().enumerate() {
        push_indent(next_indent, out);
        write_lua_value(value, next_indent, out);
        if index + 1 != values.len() {
            out.push(',');
        }
        out.push('\n');
    }
    push_indent(indent, out);
    out.push('}');
}

fn write_lua_object(
    values: &serde_json::Map<String, serde_json::Value>,
    indent: usize,
    out: &mut String,
) {
    if values.is_empty() {
        out.push_str("{}");
        return;
    }

    out.push_str("{\n");
    let next_indent = indent + 1;
    let len = values.len();
    for (index, (key, value)) in values.iter().enumerate() {
        push_indent(next_indent, out);
        if is_lua_identifier(key) {
            out.push_str(key);
        } else {
            out.push('[');
            write_lua_string(key, out);
            out.push(']');
        }
        out.push_str(" = ");
        write_lua_value(value, next_indent, out);
        if index + 1 != len {
            out.push(',');
        }
        out.push('\n');
    }
    push_indent(indent, out);
    out.push('}');
}

fn write_lua_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_ascii_graphic() || ch == ' ' => out.push(ch),
            ch => {
                use std::fmt::Write;
                write!(out, "\\u{{{:x}}}", ch as u32).expect("writing to a string should not fail");
            }
        }
    }
    out.push('"');
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

fn push_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}
