pub(crate) mod serde_format;

use std::fmt;

use ron::de;
use ron::de::SpannedError;
use ron::ser;
use ron::ser::PrettyConfig;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::serde::serde_format::SerdeFormat;

/// [`fmt::Write`] over a byte buffer.
///
/// RON's serializer writes text through [`fmt::Write`], and the caller's buffer is where that
/// text goes, so the bridge appends to it with no `String` between. Writing never fails, so no
/// error is lost by the bridge.
#[derive(Debug)]
struct Utf8Writer<'a>(&'a mut Vec<u8>);

impl fmt::Write for Utf8Writer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SerializeError {
    #[error("RON serialization failed: {0}")]
    Ron(#[from] ron::Error),
    #[error("Bitcode serialization failed: {0}")]
    Bitcode(#[from] bitcode::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    #[error("RON deserialization failed: {0}")]
    Ron(#[from] SpannedError),
    #[error("Bitcode deserialization failed: {0}")]
    Bitcode(#[from] bitcode::Error),
}

pub fn serialize<T: Serialize + ?Sized>(
    value: &T,
    format: SerdeFormat,
) -> Result<Vec<u8>, SerializeError> {
    let mut bytes = Vec::new();
    serialize_into(value, format, &mut bytes)?;
    Ok(bytes)
}

/// Appends `value`'s encoding to `out`; on an error `out` may hold part of it.
///
/// RON text goes straight into `out`. Bitcode's serde encoder builds its column buffers and the
/// finished bytes for each call, so that arm allocates whatever `out` holds.
pub fn serialize_into<T: Serialize + ?Sized>(
    value: &T,
    format: SerdeFormat,
    out: &mut Vec<u8>,
) -> Result<(), SerializeError> {
    match format {
        SerdeFormat::Ron => {
            ser::to_writer_pretty(Utf8Writer(out), value, PrettyConfig::default())?;
        }
        SerdeFormat::Bitcode => out.extend_from_slice(&bitcode::serialize(value)?),
    }
    Ok(())
}

pub fn deserialize<T: DeserializeOwned>(
    serialized: &[u8],
    format: SerdeFormat,
) -> Result<T, DeserializeError> {
    match format {
        SerdeFormat::Ron => Ok(de::from_bytes(serialized)?),
        SerdeFormat::Bitcode => Ok(bitcode::deserialize(serialized)?),
    }
}

#[cfg(test)]
mod tests;
