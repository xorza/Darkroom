use std::io::Write;
use std::sync::OnceLock;

use rhai::{Dynamic, Engine};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum SerdeRhaiError {
    #[error("Rhai evaluation failed: {0}")]
    Eval(String),
    #[error("Serialization to JSON intermediary failed")]
    SerdeJson(#[from] serde_json::Error),
    #[error("Invalid UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("IO error")]
    Io(#[from] std::io::Error),
}

pub type SerdeRhaiResult<T> = Result<T, SerdeRhaiError>;

pub fn from_slice<T: DeserializeOwned>(serialized: &[u8]) -> SerdeRhaiResult<T> {
    from_str(std::str::from_utf8(serialized)?)
}

pub fn from_reader<T: DeserializeOwned, R: std::io::Read>(reader: &mut R) -> SerdeRhaiResult<T> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    from_slice(&buf)
}

pub fn from_str<T: DeserializeOwned>(serialized: &str) -> SerdeRhaiResult<T> {
    let value: Dynamic = data_engine()
        .eval_expression(serialized.trim())
        .map_err(|e| SerdeRhaiError::Eval(e.to_string()))?;
    // Route Dynamic -> serde_json::Value -> T so numeric widths
    // (f64 -> f32, i64 -> i32/u8) are coerced leniently instead of
    // rejected by rhai::serde::from_dynamic's exact-type check.
    let json: serde_json::Value = serde_json::to_value(&value)?;
    Ok(serde_json::from_value(json)?)
}

pub fn to_string<T: Serialize>(value: &T) -> SerdeRhaiResult<String> {
    let mut out = Vec::new();
    to_writer(&mut out, value)?;
    Ok(String::from_utf8(out).expect("rhai output should be valid utf-8"))
}

pub fn to_writer<W: Write, T: Serialize>(writer: &mut W, value: &T) -> SerdeRhaiResult<()> {
    let json_value = serde_json::to_value(value)?;
    write_value(&json_value, 0, writer)?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn data_engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut engine = Engine::new_raw();
        engine.set_max_operations(10_000_000);
        engine.set_max_expr_depths(128, 32);
        engine.set_max_string_size(16 * 1024 * 1024);
        engine.set_max_array_size(1_000_000);
        engine.set_max_map_size(1_000_000);
        engine.set_max_variables(0);
        engine.set_max_modules(0);
        engine.set_max_call_levels(0);
        engine
    })
}

fn write_value<W: Write>(
    value: &serde_json::Value,
    indent: usize,
    out: &mut W,
) -> std::io::Result<()> {
    match value {
        serde_json::Value::Null => out.write_all(b"()"),
        serde_json::Value::Bool(v) => out.write_all(if *v { b"true" } else { b"false" }),
        serde_json::Value::Number(n) => write_number(n, out),
        serde_json::Value::String(s) => write_string(s, out),
        serde_json::Value::Array(values) => write_array(values, indent, out),
        serde_json::Value::Object(values) => write_object(values, indent, out),
    }
}

fn write_number<W: Write>(n: &serde_json::Number, out: &mut W) -> std::io::Result<()> {
    if let Some(i) = n.as_i64() {
        write!(out, "{}", i)
    } else if let Some(u) = n.as_u64() {
        write!(out, "{}", u)
    } else {
        // serde_json::Number is finite-only (NaN/Inf rejected at Number::from_f64),
        // so as_f64() is guaranteed Some here.
        let f = n.as_f64().expect("serde_json::Number is finite");
        // ryu always emits `.` or exponent, so the value round-trips as FLOAT
        // rather than collapsing to INT (which Rhai parses distinctly).
        let mut buf = ryu::Buffer::new();
        out.write_all(buf.format(f).as_bytes())
    }
}

fn write_array<W: Write>(
    values: &[serde_json::Value],
    indent: usize,
    out: &mut W,
) -> std::io::Result<()> {
    if values.is_empty() {
        return out.write_all(b"[]");
    }

    out.write_all(b"[\n")?;
    let next_indent = indent + 1;
    for value in values {
        push_indent(next_indent, out)?;
        write_value(value, next_indent, out)?;
        out.write_all(b",\n")?;
    }
    push_indent(indent, out)?;
    out.write_all(b"]")
}

fn write_object<W: Write>(
    values: &serde_json::Map<String, serde_json::Value>,
    indent: usize,
    out: &mut W,
) -> std::io::Result<()> {
    if values.is_empty() {
        return out.write_all(b"#{}");
    }

    out.write_all(b"#{\n")?;
    let next_indent = indent + 1;
    for (key, value) in values {
        push_indent(next_indent, out)?;
        if is_bare_identifier(key) {
            out.write_all(key.as_bytes())?;
        } else {
            write_string(key, out)?;
        }
        out.write_all(b": ")?;
        write_value(value, next_indent, out)?;
        out.write_all(b",\n")?;
    }
    push_indent(indent, out)?;
    out.write_all(b"}")
}

fn write_string<W: Write>(value: &str, out: &mut W) -> std::io::Result<()> {
    out.write_all(b"\"")?;
    let bytes = value.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let esc: &[u8] = match b {
            b'\\' => b"\\\\",
            b'"' => b"\\\"",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0..=0x1F | 0x7F => {
                out.write_all(&bytes[start..i])?;
                write!(out, "\\x{:02X}", b)?;
                start = i + 1;
                continue;
            }
            // Non-ASCII UTF-8 continuation bytes pass through; Rhai accepts
            // raw UTF-8 in string literals, so no escaping needed.
            _ => continue,
        };
        out.write_all(&bytes[start..i])?;
        out.write_all(esc)?;
        start = i + 1;
    }
    out.write_all(&bytes[start..])?;
    out.write_all(b"\"")
}

fn push_indent<W: Write>(level: usize, out: &mut W) -> std::io::Result<()> {
    for _ in 0..level {
        out.write_all(b"    ")?;
    }
    Ok(())
}

fn is_bare_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return false;
    }
    !is_rhai_keyword(s)
}

fn is_rhai_keyword(s: &str) -> bool {
    matches!(
        s,
        "true"
            | "false"
            | "let"
            | "const"
            | "fn"
            | "if"
            | "else"
            | "while"
            | "loop"
            | "for"
            | "in"
            | "do"
            | "break"
            | "continue"
            | "return"
            | "throw"
            | "try"
            | "catch"
            | "switch"
            | "import"
            | "export"
            | "as"
            | "null"
            | "void"
            | "this"
            | "private"
            | "public"
            | "new"
    )
}
