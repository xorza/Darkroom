use std::io::Write;

use super::error::Result;
use super::value::ScnValue;

pub fn emit_value<W: Write>(
    w: &mut W,
    value: &ScnValue,
    indent: usize,
    at_start: bool,
) -> Result<()> {
    match value {
        ScnValue::Null => emit_atom(w, b"null", indent, at_start),
        ScnValue::Bool(true) => emit_atom(w, b"true", indent, at_start),
        ScnValue::Bool(false) => emit_atom(w, b"false", indent, at_start),
        ScnValue::Int(i) => {
            if at_start {
                write_indent(w, indent)?;
            }
            write!(w, "{i}")?;
            Ok(())
        }
        ScnValue::Uint(u) => {
            if at_start {
                write_indent(w, indent)?;
            }
            write!(w, "{u}")?;
            Ok(())
        }
        ScnValue::Float(f) => emit_float(w, *f, indent, at_start),
        ScnValue::String(s) => emit_string(w, s, indent, at_start),
        ScnValue::Array(items) => emit_array(w, items, indent, at_start),
        ScnValue::Map(entries) => emit_map(w, entries, indent, at_start),
        ScnValue::Variant(tag, payload) => emit_variant(w, tag, payload, indent, at_start),
    }
}

fn emit_atom<W: Write>(w: &mut W, text: &[u8], indent: usize, at_start: bool) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    w.write_all(text)?;
    Ok(())
}

fn emit_float<W: Write>(w: &mut W, f: f64, indent: usize, at_start: bool) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    if f.is_nan() {
        w.write_all(b"null")?;
    } else if f.is_infinite() {
        // Represent infinities as very large/small numbers. SCN doesn't have inf literals.
        // Use null to avoid data corruption.
        w.write_all(b"null")?;
    } else {
        // Ensure float always has a decimal point so it parses back as float
        let s = format!("{f}");
        if s.contains('.') || s.contains('e') || s.contains('E') {
            w.write_all(s.as_bytes())?;
        } else {
            write!(w, "{s}.0")?;
        }
    }
    Ok(())
}

fn emit_string<W: Write>(w: &mut W, s: &str, indent: usize, at_start: bool) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    w.write_all(b"\"")?;
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let esc: Option<&[u8]> = match b {
            b'\\' => Some(b"\\\\"),
            b'"' => Some(b"\\\""),
            b'\n' => Some(b"\\n"),
            b'\r' => Some(b"\\r"),
            b'\t' => Some(b"\\t"),
            b'\0' => Some(b"\\0"),
            _ => None,
        };
        if let Some(esc) = esc {
            w.write_all(&bytes[start..i])?;
            w.write_all(esc)?;
            i += 1;
            start = i;
            continue;
        }
        // Control chars (except those already handled)
        if b < 0x20 || b == 0x7F {
            w.write_all(&bytes[start..i])?;
            write!(w, "\\u{{{:x}}}", b)?;
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }
    w.write_all(&bytes[start..])?;
    w.write_all(b"\"")?;
    Ok(())
}

fn emit_array<W: Write>(
    w: &mut W,
    items: &[ScnValue],
    indent: usize,
    at_start: bool,
) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    if items.is_empty() {
        w.write_all(b"[]")?;
        return Ok(());
    }

    // Inline small arrays of simple values
    if items.len() <= 8 && items.iter().all(is_simple) {
        w.write_all(b"[")?;
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                w.write_all(b", ")?;
            }
            emit_value(w, item, 0, false)?;
        }
        w.write_all(b"]")?;
        return Ok(());
    }

    w.write_all(b"[\n")?;
    let next = indent + 1;
    for (i, item) in items.iter().enumerate() {
        emit_value(w, item, next, true)?;
        if i + 1 < items.len() {
            w.write_all(b",")?;
        }
        w.write_all(b"\n")?;
    }
    write_indent(w, indent)?;
    w.write_all(b"]")?;
    Ok(())
}

fn emit_map<W: Write>(
    w: &mut W,
    entries: &[(String, ScnValue)],
    indent: usize,
    at_start: bool,
) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    if entries.is_empty() {
        w.write_all(b"{}")?;
        return Ok(());
    }

    w.write_all(b"{\n")?;
    let next = indent + 1;
    for (i, (key, value)) in entries.iter().enumerate() {
        write_indent(w, next)?;
        emit_key(w, key)?;
        w.write_all(b": ")?;
        emit_value(w, value, next, false)?;
        if i + 1 < entries.len() {
            w.write_all(b",")?;
        }
        w.write_all(b"\n")?;
    }
    write_indent(w, indent)?;
    w.write_all(b"}")?;
    Ok(())
}

fn emit_variant<W: Write>(
    w: &mut W,
    tag: &str,
    payload: &Option<Box<ScnValue>>,
    indent: usize,
    at_start: bool,
) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    w.write_all(tag.as_bytes())?;
    if let Some(inner) = payload {
        match inner.as_ref() {
            ScnValue::Map(_) => {
                // Struct variant: Tag { ... }
                w.write_all(b" ")?;
                emit_value(w, inner, indent, false)?;
            }
            _ => {
                // Newtype variant: Tag value
                w.write_all(b" ")?;
                emit_value(w, inner, indent, false)?;
            }
        }
    }
    Ok(())
}

fn emit_key<W: Write>(w: &mut W, key: &str) -> Result<()> {
    if is_bare_key(key) {
        w.write_all(key.as_bytes())?;
    } else {
        emit_string(w, key, 0, false)?;
    }
    Ok(())
}

fn is_bare_key(s: &str) -> bool {
    let bytes = s.as_bytes();
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
    !matches!(s, "true" | "false" | "null")
}

fn is_simple(v: &ScnValue) -> bool {
    matches!(
        v,
        ScnValue::Null
            | ScnValue::Bool(_)
            | ScnValue::Int(_)
            | ScnValue::Uint(_)
            | ScnValue::Float(_)
            | ScnValue::String(_)
            | ScnValue::Variant(_, None)
    )
}

const INDENT_BUF: &[u8; 64] = b"                                                                ";

fn write_indent<W: Write>(w: &mut W, indent: usize) -> Result<()> {
    let mut bytes = indent * 2;
    while bytes > 0 {
        let n = bytes.min(INDENT_BUF.len());
        w.write_all(&INDENT_BUF[..n])?;
        bytes -= n;
    }
    Ok(())
}
