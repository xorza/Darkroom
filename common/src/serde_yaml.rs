use std::fmt;
use std::io::Write;

use serde::Serialize;
use serde::de::{self, DeserializeOwned};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SerdeYamlError {
    #[error("{0}")]
    Message(String),
    #[error("IO error")]
    Io(#[from] std::io::Error),
    #[error("Invalid UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
}

impl de::Error for SerdeYamlError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        SerdeYamlError::Message(msg.to_string())
    }
}

impl serde::ser::Error for SerdeYamlError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        SerdeYamlError::Message(msg.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SerdeYamlError>;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn to_string<T: Serialize>(value: &T) -> Result<String> {
    let mut out = Vec::new();
    to_writer(&mut out, value)?;
    Ok(String::from_utf8(out).expect("yaml output should be valid utf-8"))
}

pub fn to_writer<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    let yaml_value = value.serialize(ValueSerializer)?;
    emit_value(writer, &yaml_value, 0, true)?;
    writer.write_all(b"\n")?;
    Ok(())
}

pub fn from_str<T: DeserializeOwned>(s: &str) -> Result<T> {
    let value = parse_yaml(s)?;
    T::deserialize(value)
}

pub fn from_reader<T: DeserializeOwned, R: std::io::Read>(reader: &mut R) -> Result<T> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    from_str(std::str::from_utf8(&buf)?)
}

// ===========================================================================
// YamlValue — intermediate representation used by both ser and de
// ===========================================================================

#[derive(Debug, Clone, PartialEq)]
enum YamlValue {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    String(String),
    Seq(Vec<YamlValue>),
    Map(Vec<(YamlValue, YamlValue)>),
}

// ===========================================================================
// Emitter: YamlValue → text
// ===========================================================================

fn emit_value<W: Write>(w: &mut W, value: &YamlValue, indent: usize, at_start: bool) -> Result<()> {
    match value {
        YamlValue::Null => emit_scalar(w, "null", indent, at_start),
        YamlValue::Bool(b) => emit_scalar(w, if *b { "true" } else { "false" }, indent, at_start),
        YamlValue::Int(i) => {
            if at_start {
                write_indent(w, indent)?;
            }
            write!(w, "{i}")?;
            Ok(())
        }
        YamlValue::Uint(u) => {
            if at_start {
                write_indent(w, indent)?;
            }
            write!(w, "{u}")?;
            Ok(())
        }
        YamlValue::Float(f) if f.is_nan() => emit_scalar(w, ".nan", indent, at_start),
        YamlValue::Float(f) if f.is_infinite() => emit_scalar(
            w,
            if f.is_sign_positive() {
                ".inf"
            } else {
                "-.inf"
            },
            indent,
            at_start,
        ),
        YamlValue::Float(f) => {
            if at_start {
                write_indent(w, indent)?;
            }
            write!(w, "{f}")?;
            Ok(())
        }
        YamlValue::String(s) => emit_string(w, s, indent, at_start),
        YamlValue::Seq(items) => emit_seq(w, items, indent, at_start),
        YamlValue::Map(entries) => emit_map(w, entries, indent, at_start),
    }
}

fn emit_scalar<W: Write>(w: &mut W, s: &str, indent: usize, at_start: bool) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    w.write_all(s.as_bytes())?;
    Ok(())
}

fn emit_string<W: Write>(w: &mut W, s: &str, indent: usize, at_start: bool) -> Result<()> {
    if at_start {
        write_indent(w, indent)?;
    }
    if needs_quoting(s) {
        w.write_all(b"\"")?;
        let bytes = s.as_bytes();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let esc: &[u8] = match b {
                b'\\' => b"\\\\",
                b'"' => b"\\\"",
                b'\n' => b"\\n",
                b'\r' => b"\\r",
                b'\t' => b"\\t",
                b'\0' => b"\\0",
                _ => continue,
            };
            w.write_all(&bytes[start..i])?;
            w.write_all(esc)?;
            start = i + 1;
        }
        w.write_all(&bytes[start..])?;
        w.write_all(b"\"")?;
    } else {
        w.write_all(s.as_bytes())?;
    }
    Ok(())
}

fn emit_seq<W: Write>(w: &mut W, items: &[YamlValue], indent: usize, at_start: bool) -> Result<()> {
    if items.is_empty() {
        if at_start {
            write_indent(w, indent)?;
        }
        w.write_all(b"[]")?;
        return Ok(());
    }

    for (i, item) in items.iter().enumerate() {
        if i > 0 || !at_start {
            w.write_all(b"\n")?;
        }
        write_indent(w, indent)?;
        w.write_all(b"- ")?;

        if is_compound(item) {
            w.write_all(b"\n")?;
            emit_value(w, item, indent + 1, true)?;
        } else {
            emit_value(w, item, indent + 1, false)?;
        }
    }
    Ok(())
}

fn emit_map<W: Write>(
    w: &mut W,
    entries: &[(YamlValue, YamlValue)],
    indent: usize,
    at_start: bool,
) -> Result<()> {
    if entries.is_empty() {
        if at_start {
            write_indent(w, indent)?;
        }
        w.write_all(b"{}")?;
        return Ok(());
    }

    for (i, (key, value)) in entries.iter().enumerate() {
        if i > 0 || !at_start {
            w.write_all(b"\n")?;
        }
        write_indent(w, indent)?;
        emit_map_key(w, key)?;
        w.write_all(b":")?;

        if is_compound(value) {
            w.write_all(b"\n")?;
            emit_value(w, value, indent + 1, true)?;
        } else {
            w.write_all(b" ")?;
            emit_value(w, value, indent + 1, false)?;
        }
    }
    Ok(())
}

fn emit_map_key<W: Write>(w: &mut W, key: &YamlValue) -> Result<()> {
    match key {
        YamlValue::String(s) => emit_string(w, s, 0, false),
        YamlValue::Int(i) => {
            write!(w, "{i}")?;
            Ok(())
        }
        YamlValue::Uint(u) => {
            write!(w, "{u}")?;
            Ok(())
        }
        YamlValue::Bool(b) => {
            w.write_all(if *b { b"true" } else { b"false" })?;
            Ok(())
        }
        _ => Err(SerdeYamlError::Message(format!(
            "unsupported map key type: {:?}",
            key
        ))),
    }
}

fn is_compound(v: &YamlValue) -> bool {
    match v {
        YamlValue::Seq(items) => !items.is_empty(),
        YamlValue::Map(entries) => !entries.is_empty(),
        _ => false,
    }
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

fn needs_quoting(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return true;
    }
    if bytes[0] == b' ' || bytes[bytes.len() - 1] == b' ' {
        return true;
    }
    match s {
        "null" | "Null" | "NULL" | "~" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
        | "yes" | "Yes" | "YES" | "no" | "No" | "NO" | "on" | "On" | "ON" | "off" | "Off"
        | "OFF" => return true,
        _ => {}
    }
    if looks_like_number(s) {
        return true;
    }
    if matches!(bytes[0], b'-' | b'?' | b'.') {
        return true;
    }
    for &b in bytes {
        if matches!(
            b,
            b':' | b'#'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b','
                | b'&'
                | b'*'
                | b'!'
                | b'|'
                | b'>'
                | b'\''
                | b'"'
                | b'%'
                | b'@'
                | b'`'
                | b'\n'
                | b'\r'
                | b'\t'
                | b'\0'
        ) {
            return true;
        }
    }
    false
}

fn looks_like_number(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let start = if bytes[0] == b'-' || bytes[0] == b'+' {
        1
    } else {
        0
    };
    if start >= bytes.len() {
        return false;
    }
    let mut has_digit = false;
    for &b in &bytes[start..] {
        match b {
            b'0'..=b'9' => has_digit = true,
            b'.' | b'e' | b'E' | b'+' | b'-' => {}
            _ => return false,
        }
    }
    has_digit
}

// ===========================================================================
// Serializer: T → YamlValue (tree-building, no lifetime issues)
// ===========================================================================

struct ValueSerializer;

impl serde::Serializer for ValueSerializer {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    type SerializeSeq = SeqBuilder;
    type SerializeTuple = SeqBuilder;
    type SerializeTupleStruct = SeqBuilder;
    type SerializeTupleVariant = TupleVariantBuilder;
    type SerializeMap = MapBuilder;
    type SerializeStruct = MapBuilder;
    type SerializeStructVariant = StructVariantBuilder;

    fn serialize_bool(self, v: bool) -> Result<YamlValue> {
        Ok(YamlValue::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<YamlValue> {
        Ok(YamlValue::Int(v as i64))
    }
    fn serialize_i16(self, v: i16) -> Result<YamlValue> {
        Ok(YamlValue::Int(v as i64))
    }
    fn serialize_i32(self, v: i32) -> Result<YamlValue> {
        Ok(YamlValue::Int(v as i64))
    }
    fn serialize_i64(self, v: i64) -> Result<YamlValue> {
        Ok(YamlValue::Int(v))
    }
    fn serialize_u8(self, v: u8) -> Result<YamlValue> {
        Ok(YamlValue::Uint(v as u64))
    }
    fn serialize_u16(self, v: u16) -> Result<YamlValue> {
        Ok(YamlValue::Uint(v as u64))
    }
    fn serialize_u32(self, v: u32) -> Result<YamlValue> {
        Ok(YamlValue::Uint(v as u64))
    }
    fn serialize_u64(self, v: u64) -> Result<YamlValue> {
        Ok(YamlValue::Uint(v))
    }
    fn serialize_f32(self, v: f32) -> Result<YamlValue> {
        Ok(YamlValue::Float(v as f64))
    }
    fn serialize_f64(self, v: f64) -> Result<YamlValue> {
        Ok(YamlValue::Float(v))
    }
    fn serialize_char(self, v: char) -> Result<YamlValue> {
        Ok(YamlValue::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<YamlValue> {
        Ok(YamlValue::String(v.to_string()))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<YamlValue> {
        Err(SerdeYamlError::Message(
            "byte arrays not supported".to_string(),
        ))
    }
    fn serialize_none(self) -> Result<YamlValue> {
        Ok(YamlValue::Null)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<YamlValue> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<YamlValue> {
        Ok(YamlValue::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<YamlValue> {
        Ok(YamlValue::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<YamlValue> {
        Ok(YamlValue::String(variant.to_string()))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<YamlValue> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<YamlValue> {
        let inner = value.serialize(ValueSerializer)?;
        Ok(YamlValue::Map(vec![(
            YamlValue::String(variant.to_string()),
            inner,
        )]))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqBuilder> {
        Ok(SeqBuilder {
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqBuilder> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SeqBuilder> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<TupleVariantBuilder> {
        Ok(TupleVariantBuilder {
            variant: variant.to_string(),
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapBuilder> {
        Ok(MapBuilder {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            current_key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<MapBuilder> {
        self.serialize_map(Some(len))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<StructVariantBuilder> {
        Ok(StructVariantBuilder {
            variant: variant.to_string(),
            entries: Vec::with_capacity(len),
        })
    }
}

// ---------------------------------------------------------------------------
// Sequence builder
// ---------------------------------------------------------------------------

struct SeqBuilder {
    items: Vec<YamlValue>,
}

impl serde::ser::SerializeSeq for SeqBuilder {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<YamlValue> {
        Ok(YamlValue::Seq(self.items))
    }
}

impl serde::ser::SerializeTuple for SeqBuilder {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<YamlValue> {
        serde::ser::SerializeSeq::end(self)
    }
}

impl serde::ser::SerializeTupleStruct for SeqBuilder {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<YamlValue> {
        serde::ser::SerializeSeq::end(self)
    }
}

// ---------------------------------------------------------------------------
// Tuple variant builder
// ---------------------------------------------------------------------------

struct TupleVariantBuilder {
    variant: String,
    items: Vec<YamlValue>,
}

impl serde::ser::SerializeTupleVariant for TupleVariantBuilder {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<YamlValue> {
        Ok(YamlValue::Map(vec![(
            YamlValue::String(self.variant),
            YamlValue::Seq(self.items),
        )]))
    }
}

// ---------------------------------------------------------------------------
// Map builder
// ---------------------------------------------------------------------------

struct MapBuilder {
    entries: Vec<(YamlValue, YamlValue)>,
    current_key: Option<YamlValue>,
}

impl serde::ser::SerializeMap for MapBuilder {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> {
        self.current_key = Some(key.serialize(ValueSerializer)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let key = self
            .current_key
            .take()
            .expect("serialize_value called before serialize_key");
        self.entries.push((key, value.serialize(ValueSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<YamlValue> {
        Ok(YamlValue::Map(self.entries))
    }
}

impl serde::ser::SerializeStruct for MapBuilder {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries.push((
            YamlValue::String(key.to_string()),
            value.serialize(ValueSerializer)?,
        ));
        Ok(())
    }
    fn end(self) -> Result<YamlValue> {
        Ok(YamlValue::Map(self.entries))
    }
}

// ---------------------------------------------------------------------------
// Struct variant builder
// ---------------------------------------------------------------------------

struct StructVariantBuilder {
    variant: String,
    entries: Vec<(YamlValue, YamlValue)>,
}

impl serde::ser::SerializeStructVariant for StructVariantBuilder {
    type Ok = YamlValue;
    type Error = SerdeYamlError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries.push((
            YamlValue::String(key.to_string()),
            value.serialize(ValueSerializer)?,
        ));
        Ok(())
    }
    fn end(self) -> Result<YamlValue> {
        Ok(YamlValue::Map(vec![(
            YamlValue::String(self.variant),
            YamlValue::Map(self.entries),
        )]))
    }
}

// ===========================================================================
// Parser: text → YamlValue tree
// ===========================================================================

fn parse_yaml(input: &str) -> Result<YamlValue> {
    let lines = tokenize(input);
    if lines.is_empty() {
        return Ok(YamlValue::Null);
    }
    let (value, _) = parse_value(&lines, 0)?;
    Ok(value)
}

#[derive(Debug)]
struct Line<'a> {
    indent: usize,
    content: &'a str,
}

fn tokenize(input: &str) -> Vec<Line<'_>> {
    input
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_end();
            let stripped = trimmed.trim_start();
            if stripped.is_empty() || stripped.starts_with('#') {
                return None;
            }
            let indent = trimmed.len() - stripped.len();
            Some(Line {
                indent,
                content: stripped,
            })
        })
        .collect()
}

fn parse_value(lines: &[Line<'_>], start: usize) -> Result<(YamlValue, usize)> {
    if start >= lines.len() {
        return Ok((YamlValue::Null, start));
    }

    let line = &lines[start];

    if line.content.starts_with("- ") || line.content == "-" {
        return parse_sequence(lines, start, line.indent);
    }

    if find_colon_in_key(line.content).is_some() {
        return parse_map(lines, start, line.indent);
    }

    let val = parse_scalar(line.content);
    Ok((val, start + 1))
}

fn parse_sequence(
    lines: &[Line<'_>],
    start: usize,
    base_indent: usize,
) -> Result<(YamlValue, usize)> {
    let mut items = Vec::new();
    let mut pos = start;

    while pos < lines.len()
        && lines[pos].indent == base_indent
        && (lines[pos].content.starts_with("- ") || lines[pos].content == "-")
    {
        let line = &lines[pos];
        let after_dash = if line.content == "-" {
            ""
        } else {
            &line.content[2..]
        };

        if after_dash.is_empty() {
            pos += 1;
            if pos < lines.len() && lines[pos].indent > base_indent {
                let (val, next) = parse_value(lines, pos)?;
                items.push(val);
                pos = next;
            } else {
                items.push(YamlValue::Null);
            }
        } else if find_colon_in_key(after_dash).is_some() {
            // Inline map starting on same line as dash
            let mut sub_lines: Vec<Line<'_>> = Vec::new();
            let inner_indent = base_indent + 2;
            sub_lines.push(Line {
                indent: inner_indent,
                content: after_dash,
            });
            pos += 1;
            while pos < lines.len() && lines[pos].indent > base_indent {
                sub_lines.push(Line {
                    indent: lines[pos].indent,
                    content: lines[pos].content,
                });
                pos += 1;
            }
            let (val, _) = parse_value(&sub_lines, 0)?;
            items.push(val);
        } else {
            items.push(parse_scalar(after_dash));
            pos += 1;
        }
    }

    Ok((YamlValue::Seq(items), pos))
}

fn parse_map(lines: &[Line<'_>], start: usize, base_indent: usize) -> Result<(YamlValue, usize)> {
    let mut entries = Vec::new();
    let mut pos = start;

    while pos < lines.len() && lines[pos].indent == base_indent {
        let line = &lines[pos];

        let colon_pos = match find_colon_in_key(line.content) {
            Some(p) => p,
            None => break,
        };

        let raw_key = &line.content[..colon_pos];
        let key = parse_scalar(raw_key);
        let after_colon = line.content[colon_pos + 1..].trim_start();

        if after_colon.is_empty() {
            pos += 1;
            if pos < lines.len() && lines[pos].indent > base_indent {
                let (val, next) = parse_value(lines, pos)?;
                entries.push((key, val));
                pos = next;
            } else {
                entries.push((key, YamlValue::Null));
            }
        } else {
            entries.push((key, parse_scalar(after_colon)));
            pos += 1;
        }
    }

    Ok((YamlValue::Map(entries), pos))
}

/// Find the colon that separates key from value.
fn find_colon_in_key(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;

    if bytes.first() == Some(&b'"') {
        i = 1;
        while i < bytes.len() {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
                continue;
            }
            if bytes[i] == b'"' {
                i += 1;
                break;
            }
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b':' {
            return Some(i);
        }
        return None;
    }

    while i < bytes.len() {
        if bytes[i] == b':' && (i + 1 >= bytes.len() || bytes[i + 1] == b' ') {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_scalar(s: &str) -> YamlValue {
    let s = s.trim();
    if s.is_empty() {
        return YamlValue::Null;
    }
    match s {
        "null" | "Null" | "NULL" | "~" => YamlValue::Null,
        "true" | "True" | "TRUE" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => {
            YamlValue::Bool(true)
        }
        "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => {
            YamlValue::Bool(false)
        }
        "[]" => YamlValue::Seq(Vec::new()),
        "{}" => YamlValue::Map(Vec::new()),
        ".nan" | ".NaN" | ".NAN" => YamlValue::Float(f64::NAN),
        ".inf" | ".Inf" | ".INF" => YamlValue::Float(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => YamlValue::Float(f64::NEG_INFINITY),
        _ => {
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                return YamlValue::String(unescape_quoted(&s[1..s.len() - 1]));
            }
            if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
                return YamlValue::String(s[1..s.len() - 1].to_string());
            }
            if let Ok(v) = s.parse::<i64>() {
                return YamlValue::Int(v);
            }
            if let Ok(v) = s.parse::<u64>() {
                return YamlValue::Uint(v);
            }
            if let Ok(v) = s.parse::<f64>()
                && (s.contains('.') || s.contains('e') || s.contains('E'))
            {
                return YamlValue::Float(v);
            }
            YamlValue::String(s.to_string())
        }
    }
}

fn unescape_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('0') => out.push('\0'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

// ===========================================================================
// Deserializer: YamlValue → T
// ===========================================================================

impl<'de> de::Deserializer<'de> for YamlValue {
    type Error = SerdeYamlError;

    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Null => visitor.visit_unit(),
            YamlValue::Bool(b) => visitor.visit_bool(b),
            YamlValue::Int(i) => visitor.visit_i64(i),
            YamlValue::Uint(u) => visitor.visit_u64(u),
            YamlValue::Float(f) => visitor.visit_f64(f),
            YamlValue::String(s) => visitor.visit_string(s),
            YamlValue::Seq(seq) => {
                let mut de = SeqDeserializer {
                    iter: seq.into_iter(),
                };
                visitor.visit_seq(&mut de)
            }
            YamlValue::Map(map) => {
                let mut de = MapDeserializer {
                    iter: map.into_iter(),
                    current_value: None,
                };
                visitor.visit_map(&mut de)
            }
        }
    }

    fn deserialize_bool<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Bool(b) => visitor.visit_bool(b),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_i8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Int(i) => visitor.visit_i64(i),
            YamlValue::Uint(u) => visitor.visit_u64(u),
            YamlValue::Float(f) => visitor.visit_f64(f),
            _ => self.deserialize_any(visitor),
        }
    }
    fn deserialize_u8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Uint(u) => visitor.visit_u64(u),
            YamlValue::Int(i) => visitor.visit_i64(i),
            YamlValue::Float(f) => visitor.visit_f64(f),
            _ => self.deserialize_any(visitor),
        }
    }
    fn deserialize_f32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_f64(visitor)
    }
    fn deserialize_f64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Float(f) => visitor.visit_f64(f),
            YamlValue::Int(i) => visitor.visit_f64(i as f64),
            YamlValue::Uint(u) => visitor.visit_f64(u as f64),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_char<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_str<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::String(s) => visitor.visit_string(s),
            YamlValue::Int(i) => visitor.visit_string(i.to_string()),
            YamlValue::Uint(u) => visitor.visit_string(u.to_string()),
            YamlValue::Float(f) => visitor.visit_string(f.to_string()),
            YamlValue::Bool(b) => visitor.visit_string(b.to_string()),
            YamlValue::Null => visitor.visit_string("null".to_string()),
            _ => Err(SerdeYamlError::Message(format!(
                "expected string, got {:?}",
                self
            ))),
        }
    }

    fn deserialize_string<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: de::Visitor<'de>>(self, _visitor: V) -> Result<V::Value> {
        Err(SerdeYamlError::Message(
            "byte arrays not supported".to_string(),
        ))
    }
    fn deserialize_byte_buf<V: de::Visitor<'de>>(self, _visitor: V) -> Result<V::Value> {
        Err(SerdeYamlError::Message(
            "byte arrays not supported".to_string(),
        ))
    }

    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Null => visitor.visit_unit(),
            _ => Err(SerdeYamlError::Message(format!(
                "expected null, got {:?}",
                self
            ))),
        }
    }

    fn deserialize_unit_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Seq(seq) => {
                let mut de = SeqDeserializer {
                    iter: seq.into_iter(),
                };
                visitor.visit_seq(&mut de)
            }
            _ => Err(SerdeYamlError::Message(format!(
                "expected sequence, got {:?}",
                self
            ))),
        }
    }

    fn deserialize_tuple<V: de::Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            YamlValue::Map(map) => {
                let mut de = MapDeserializer {
                    iter: map.into_iter(),
                    current_value: None,
                };
                visitor.visit_map(&mut de)
            }
            _ => Err(SerdeYamlError::Message(format!(
                "expected map, got {:?}",
                self
            ))),
        }
    }

    fn deserialize_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self {
            YamlValue::String(s) => visitor.visit_enum(EnumDeserializer {
                variant: s,
                value: None,
            }),
            YamlValue::Map(mut map) if map.len() == 1 => {
                let (variant_key, value) = map.remove(0);
                let variant = match variant_key {
                    YamlValue::String(s) => s,
                    _ => {
                        return Err(SerdeYamlError::Message(format!(
                            "expected string enum variant key, got {:?}",
                            variant_key
                        )));
                    }
                };
                visitor.visit_enum(EnumDeserializer {
                    variant,
                    value: Some(value),
                })
            }
            _ => Err(SerdeYamlError::Message(format!(
                "expected string or single-key map for enum, got {:?}",
                self
            ))),
        }
    }

    fn deserialize_identifier<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_unit()
    }
}

// ---------------------------------------------------------------------------
// Sequence deserializer
// ---------------------------------------------------------------------------

struct SeqDeserializer {
    iter: std::vec::IntoIter<YamlValue>,
}

impl<'de> de::SeqAccess<'de> for SeqDeserializer {
    type Error = SerdeYamlError;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>> {
        match self.iter.next() {
            Some(value) => seed.deserialize(value).map(Some),
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Map deserializer
// ---------------------------------------------------------------------------

struct MapDeserializer {
    iter: std::vec::IntoIter<(YamlValue, YamlValue)>,
    current_value: Option<YamlValue>,
}

impl<'de> de::MapAccess<'de> for MapDeserializer {
    type Error = SerdeYamlError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        match self.iter.next() {
            Some((key, value)) => {
                self.current_value = Some(value);
                seed.deserialize(key).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let value = self
            .current_value
            .take()
            .expect("next_value_seed called before next_key_seed");
        seed.deserialize(value)
    }
}

// ---------------------------------------------------------------------------
// Enum deserializer
// ---------------------------------------------------------------------------

struct EnumDeserializer {
    variant: String,
    value: Option<YamlValue>,
}

impl<'de> de::EnumAccess<'de> for EnumDeserializer {
    type Error = SerdeYamlError;
    type Variant = VariantDeserializer;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(YamlValue::String(self.variant))?;
        Ok((variant, VariantDeserializer { value: self.value }))
    }
}

struct VariantDeserializer {
    value: Option<YamlValue>,
}

impl<'de> de::VariantAccess<'de> for VariantDeserializer {
    type Error = SerdeYamlError;

    fn unit_variant(self) -> Result<()> {
        if self.value.is_some() {
            return Err(SerdeYamlError::Message(
                "expected unit variant, got value".to_string(),
            ));
        }
        Ok(())
    }

    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        match self.value {
            Some(value) => seed.deserialize(value),
            None => Err(SerdeYamlError::Message(
                "expected newtype variant value".to_string(),
            )),
        }
    }

    fn tuple_variant<V: de::Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        match self.value {
            Some(YamlValue::Seq(seq)) => {
                let mut de = SeqDeserializer {
                    iter: seq.into_iter(),
                };
                visitor.visit_seq(&mut de)
            }
            Some(other) => Err(SerdeYamlError::Message(format!(
                "expected sequence for tuple variant, got {:?}",
                other
            ))),
            None => Err(SerdeYamlError::Message(
                "expected tuple variant value".to_string(),
            )),
        }
    }

    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self.value {
            Some(YamlValue::Map(map)) => {
                let mut de = MapDeserializer {
                    iter: map.into_iter(),
                    current_value: None,
                };
                visitor.visit_map(&mut de)
            }
            Some(other) => Err(SerdeYamlError::Message(format!(
                "expected map for struct variant, got {:?}",
                other
            ))),
            None => Err(SerdeYamlError::Message(
                "expected struct variant value".to_string(),
            )),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + fmt::Debug>(value: &T) {
        let yaml = to_string(value).unwrap();
        let deserialized: T = from_str(&yaml).unwrap();
        assert_eq!(*value, deserialized, "roundtrip failed for yaml:\n{yaml}");
    }

    #[test]
    fn primitives() {
        roundtrip(&true);
        roundtrip(&false);
        roundtrip(&42i32);
        roundtrip(&-7i64);
        roundtrip(&1.234f64);
        roundtrip(&0u64);
    }

    #[test]
    fn strings() {
        roundtrip(&"hello world".to_string());
        roundtrip(&"".to_string());
        roundtrip(&"has: colon".to_string());
        roundtrip(&"has # hash".to_string());
        roundtrip(&"true".to_string());
        roundtrip(&"null".to_string());
        roundtrip(&"42".to_string());
        roundtrip(&"line\nnewline".to_string());
        roundtrip(&" leading space".to_string());
    }

    #[test]
    fn option() {
        roundtrip(&Some(42i32));
        roundtrip(&None::<i32>);
        roundtrip(&Some("hello".to_string()));
    }

    #[test]
    fn vec_of_primitives() {
        roundtrip(&vec![1i32, 2, 3]);
        roundtrip(&Vec::<i32>::new());
        roundtrip(&vec!["a".to_string(), "b".to_string()]);
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

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct WithOptional {
        required: String,
        #[serde(default)]
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

    #[test]
    fn hashmap() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert("key1".to_string(), 1i32);
        map.insert("key2".to_string(), 2);
        let yaml = to_string(&map).unwrap();
        let deserialized: HashMap<String, i32> = from_str(&yaml).unwrap();
        assert_eq!(map, deserialized);
    }

    #[test]
    fn special_float_values() {
        let yaml = to_string(&f64::INFINITY).unwrap();
        assert!(yaml.contains(".inf"));
        let v: f64 = from_str(&yaml).unwrap();
        assert!(v.is_infinite() && v.is_sign_positive());

        let yaml = to_string(&f64::NEG_INFINITY).unwrap();
        assert!(yaml.contains("-.inf"));
        let v: f64 = from_str(&yaml).unwrap();
        assert!(v.is_infinite() && v.is_sign_negative());

        let yaml = to_string(&f64::NAN).unwrap();
        assert!(yaml.contains(".nan"));
        let v: f64 = from_str(&yaml).unwrap();
        assert!(v.is_nan());
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
    fn negative_zero() {
        let yaml = to_string(&-0.0f64).unwrap();
        let v: f64 = from_str(&yaml).unwrap();
        assert_eq!(v, 0.0);
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Id(u64);

    #[test]
    fn newtype_struct() {
        roundtrip(&Id(42));
        roundtrip(&Id(0));
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    enum TupleVariant {
        Pair(i32, String),
    }

    #[test]
    fn tuple_variant() {
        roundtrip(&TupleVariant::Pair(1, "hello".to_string()));
    }

    #[test]
    fn comments_ignored() {
        let yaml = "# comment\nname: hello\n# another comment\nvalue: 42\nflag: true\n";
        let s: Simple = from_str(yaml).unwrap();
        assert_eq!(s.name, "hello");
        assert_eq!(s.value, 42);
        assert!(s.flag);
    }

    #[test]
    fn yaml_bool_aliases() {
        // yes/no/on/off should parse as bools
        assert_eq!(parse_scalar("yes"), YamlValue::Bool(true));
        assert_eq!(parse_scalar("Yes"), YamlValue::Bool(true));
        assert_eq!(parse_scalar("on"), YamlValue::Bool(true));
        assert_eq!(parse_scalar("no"), YamlValue::Bool(false));
        assert_eq!(parse_scalar("No"), YamlValue::Bool(false));
        assert_eq!(parse_scalar("off"), YamlValue::Bool(false));
    }

    #[test]
    fn single_quoted_string_parse() {
        assert_eq!(
            parse_scalar("'hello world'"),
            YamlValue::String("hello world".to_string())
        );
        assert_eq!(
            parse_scalar("'has: colon'"),
            YamlValue::String("has: colon".to_string())
        );
    }

    #[test]
    fn escape_sequences() {
        assert_eq!(unescape_quoted(r#"hello\nworld"#), "hello\nworld");
        assert_eq!(unescape_quoted(r#"tab\there"#), "tab\there");
        assert_eq!(unescape_quoted(r#"quote\"end"#), "quote\"end");
        assert_eq!(unescape_quoted(r#"back\\slash"#), "back\\slash");
        assert_eq!(unescape_quoted(r#"null\0byte"#), "null\0byte");
    }

    #[test]
    fn strings_needing_quoting_roundtrip() {
        // YAML keywords as string values
        roundtrip(&"yes".to_string());
        roundtrip(&"no".to_string());
        roundtrip(&"on".to_string());
        roundtrip(&"off".to_string());
        roundtrip(&"Yes".to_string());
        roundtrip(&"No".to_string());

        // Special characters
        roundtrip(&"key: value".to_string());
        roundtrip(&"[brackets]".to_string());
        roundtrip(&"{braces}".to_string());
        roundtrip(&"with, comma".to_string());
        roundtrip(&"trailing space ".to_string());
        roundtrip(&"-dash".to_string());
        roundtrip(&"?question".to_string());
        roundtrip(&".dot".to_string());
        roundtrip(&"tab\there".to_string());
        roundtrip(&"null\0byte".to_string());
    }

    #[test]
    fn unicode_roundtrip() {
        roundtrip(&"hello 世界".to_string());
        roundtrip(&"émojis 🎉🌍".to_string());
        roundtrip(&"Ü ö ä".to_string());
    }

    #[test]
    fn quoted_key_with_colon() {
        let yaml = "\"key:with:colons\": 42\n";
        let map: std::collections::HashMap<String, i32> = from_str(yaml).unwrap();
        assert_eq!(map["key:with:colons"], 42);
    }

    #[test]
    fn find_colon_trailing_backslash_no_panic() {
        // Malformed quoted key ending with backslash should not panic
        let result = find_colon_in_key("\"key\\");
        assert_eq!(result, None);
    }

    #[test]
    fn needs_quoting_coverage() {
        assert!(needs_quoting(""));
        assert!(needs_quoting(" leading"));
        assert!(needs_quoting("trailing "));
        assert!(needs_quoting("null"));
        assert!(needs_quoting("true"));
        assert!(needs_quoting("false"));
        assert!(needs_quoting("42"));
        assert!(needs_quoting("3.14"));
        assert!(needs_quoting("-1"));
        assert!(needs_quoting("-dash"));
        assert!(needs_quoting("?question"));
        assert!(needs_quoting(".dot"));
        assert!(needs_quoting("has:colon"));
        assert!(needs_quoting("has#hash"));

        assert!(!needs_quoting("hello"));
        assert!(!needs_quoting("simple_key"));
        assert!(!needs_quoting("CamelCase"));
        assert!(!needs_quoting("with spaces"));
    }

    #[test]
    fn looks_like_number_coverage() {
        assert!(looks_like_number("42"));
        assert!(looks_like_number("-7"));
        assert!(looks_like_number("3.14"));
        assert!(looks_like_number("1e10"));
        assert!(looks_like_number("+5"));
        assert!(looks_like_number("0"));

        assert!(!looks_like_number(""));
        assert!(!looks_like_number("abc"));
        assert!(!looks_like_number("-"));
        assert!(!looks_like_number("+"));
        assert!(!looks_like_number("12abc"));
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
    fn empty_input() {
        let v: Option<i32> = from_str("").unwrap();
        assert_eq!(v, None);

        let v: Option<i32> = from_str("  \n\n  \n").unwrap();
        assert_eq!(v, None);
    }

    #[test]
    fn serde_default_missing_field() {
        let yaml = "required: hello\n";
        let w: WithOptional = from_str(yaml).unwrap();
        assert_eq!(w.required, "hello");
        assert_eq!(w.optional, None);
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct DeepNest {
        child: Option<Box<DeepNest>>,
        value: i32,
    }

    #[test]
    fn deep_nesting() {
        let mut v = DeepNest {
            child: None,
            value: 0,
        };
        for i in 1..20 {
            v = DeepNest {
                child: Some(Box::new(v)),
                value: i,
            };
        }
        roundtrip(&v);
    }

    #[test]
    fn empty_map_and_seq_inline() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Both {
            items: Vec<i32>,
            #[serde(default)]
            extra: Option<String>,
        }
        roundtrip(&Both {
            items: vec![],
            extra: None,
        });
    }

    #[test]
    fn map_with_integer_keys() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert(1u32, "one".to_string());
        map.insert(2, "two".to_string());
        let yaml = to_string(&map).unwrap();
        let deserialized: HashMap<u32, String> = from_str(&yaml).unwrap();
        assert_eq!(map, deserialized);
    }

    #[test]
    fn top_level_sequence() {
        roundtrip(&vec![1i32, 2, 3]);
        roundtrip(&vec!["hello".to_string(), "world".to_string()]);
        roundtrip(&vec![Simple {
            name: "a".to_string(),
            value: 1,
            flag: true,
        }]);
    }

    #[test]
    fn nested_sequences() {
        roundtrip(&vec![vec![1i32, 2], vec![3, 4]]);
        roundtrip(&vec![vec![vec![1i32]]]);
        roundtrip(&vec![Vec::<i32>::new(), vec![1]]);
    }

    #[test]
    fn map_null_value() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct NullField {
            present: i32,
            #[serde(default)]
            absent: Option<String>,
        }
        // Parse YAML where a key has no value (implies null)
        let yaml = "present: 42\nabsent:\n";
        let v: NullField = from_str(yaml).unwrap();
        assert_eq!(v.present, 42);
        assert_eq!(v.absent, None);
    }

    #[test]
    fn sequence_with_block_items() {
        // Bare dash followed by indented block
        let yaml = "-\n  name: alice\n  value: 1\n  flag: true\n-\n  name: bob\n  value: 2\n  flag: false\n";
        let v: Vec<Simple> = from_str(yaml).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "alice");
        assert_eq!(v[1].name, "bob");
    }

    #[test]
    fn char_roundtrip() {
        roundtrip(&'a');
        roundtrip(&'Z');
        roundtrip(&'0');
    }

    #[test]
    fn f32_roundtrip() {
        roundtrip(&1.5f32);
        roundtrip(&-0.25f32);
        roundtrip(&0.0f32);
    }

    #[test]
    fn small_int_types() {
        roundtrip(&42u8);
        roundtrip(&-7i8);
        roundtrip(&1000u16);
        roundtrip(&-1000i16);
    }

    #[test]
    fn map_with_bool_keys() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert(true, "yes_val".to_string());
        map.insert(false, "no_val".to_string());
        let yaml = to_string(&map).unwrap();
        let deserialized: HashMap<bool, String> = from_str(&yaml).unwrap();
        assert_eq!(map, deserialized);
    }

    #[test]
    fn tilde_null() {
        let yaml = "~\n";
        let v: Option<i32> = from_str(yaml).unwrap();
        assert_eq!(v, None);
    }

    #[test]
    fn special_float_case_variants() {
        // Parser should handle multiple case forms
        assert!(matches!(parse_scalar(".NaN"), YamlValue::Float(f) if f.is_nan()));
        assert!(matches!(parse_scalar(".NAN"), YamlValue::Float(f) if f.is_nan()));
        assert_eq!(parse_scalar(".Inf"), YamlValue::Float(f64::INFINITY));
        assert_eq!(parse_scalar(".INF"), YamlValue::Float(f64::INFINITY));
        assert_eq!(parse_scalar("-.Inf"), YamlValue::Float(f64::NEG_INFINITY));
        assert_eq!(parse_scalar("-.INF"), YamlValue::Float(f64::NEG_INFINITY));
    }

    #[test]
    fn string_with_backslash_roundtrip() {
        roundtrip(&"path\\to\\file".to_string());
        roundtrip(&"escape\\nnotanewline".to_string());
    }

    #[test]
    fn string_with_quote_roundtrip() {
        roundtrip(&"say \"hello\"".to_string());
        roundtrip(&"it's".to_string());
    }

    #[test]
    fn btreemap_ordered() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert("alpha".to_string(), 1i32);
        map.insert("beta".to_string(), 2);
        map.insert("gamma".to_string(), 3);
        roundtrip(&map);
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Pair(i32, String);

    #[test]
    fn tuple_struct() {
        roundtrip(&Pair(42, "hello".to_string()));
    }

    #[test]
    fn serde_rename() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Renamed {
            #[serde(rename = "full_name")]
            name: String,
            #[serde(rename = "age_years")]
            age: u32,
        }
        roundtrip(&Renamed {
            name: "Alice".to_string(),
            age: 30,
        });
    }

    #[test]
    fn map_with_complex_values() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert("items".to_string(), vec![1i32, 2, 3]);
        map.insert("empty".to_string(), vec![]);
        roundtrip(&map);
    }

    #[test]
    fn all_none_options() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct AllOpt {
            #[serde(default)]
            a: Option<i32>,
            #[serde(default)]
            b: Option<String>,
            #[serde(default)]
            c: Option<bool>,
        }
        roundtrip(&AllOpt {
            a: None,
            b: None,
            c: None,
        });
    }

    #[test]
    fn unit_struct() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Marker;
        // Unit struct serializes as null
        let yaml = to_string(&Marker).unwrap();
        assert!(yaml.trim() == "null");
    }

    #[test]
    fn enum_in_map_value() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert("a".to_string(), Binding::None);
        map.insert("b".to_string(), Binding::Const(5));
        map.insert(
            "c".to_string(),
            Binding::Named {
                id: 1,
                label: "x".to_string(),
            },
        );
        roundtrip(&map);
    }
}
