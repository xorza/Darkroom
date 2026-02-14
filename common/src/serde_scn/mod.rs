mod de;
mod emit;
mod error;
mod parse;
mod value;

use std::io::Write;

use serde::Serialize;
use serde::de::DeserializeOwned;

pub use error::{Result, ScnError};
pub use value::ScnValue;

pub fn to_value<T: Serialize>(value: &T) -> Result<ScnValue> {
    value.serialize(value::ValueSerializer)
}

pub fn from_value<T: DeserializeOwned>(value: ScnValue) -> Result<T> {
    T::deserialize(value)
}

pub fn to_string<T: Serialize>(value: &T) -> Result<String> {
    let mut out = Vec::new();
    to_writer(&mut out, value)?;
    Ok(String::from_utf8(out).expect("scn output should be valid utf-8"))
}

pub fn to_writer<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    let scn_value = value.serialize(value::ValueSerializer)?;
    emit::emit_value(writer, &scn_value, 0, true)?;
    writer.write_all(b"\n")?;
    Ok(())
}

pub fn from_str<T: DeserializeOwned>(s: &str) -> Result<T> {
    let value = parse::parse(s)?;
    T::deserialize(value)
}

pub fn from_slice<T: DeserializeOwned>(data: &[u8]) -> Result<T> {
    let s = std::str::from_utf8(data)?;
    from_str(s)
}

pub fn from_reader<T: DeserializeOwned, R: std::io::Read>(reader: &mut R) -> Result<T> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;
    from_slice(&buf)
}

#[cfg(test)]
mod tests;
