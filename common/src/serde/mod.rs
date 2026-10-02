use std::fmt;
use std::io::{Read, Write};

use ron::ser::PrettyConfig;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_format::SerdeFormat;
use lz4_flex::block;
use lz4_flex::block::CompressError;
use lz4_flex::block::DecompressError;
use ron::de;
use ron::de::SpannedError;
use ron::ser;
use std::io;

const LZ4_HEADER_LEN: usize = size_of::<u32>();
const LZ4_MAX_UNCOMPRESSED_SIZE: usize = 1 << 30;

#[derive(Debug)]
struct Lz4Payload<'a> {
    compressed: &'a [u8],
    uncompressed_size: usize,
}

/// [`fmt::Write`] over a byte buffer.
///
/// RON's serializer writes text through [`fmt::Write`], and both arms that use
/// it already own reusable byte scratch they must not trade for a fresh
/// `String` per call. Writing never fails, so no error is lost by the bridge.
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
    #[error("LZ4 compression failed: {0}")]
    Lz4(#[from] CompressError),
    #[error("writing serialized bytes failed: {0}")]
    Write(#[from] io::Error),
    #[error(transparent)]
    Lz4Size(#[from] Lz4SizeError),
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    #[error("RON deserialization failed: {0}")]
    Ron(#[from] SpannedError),
    #[error("Bitcode deserialization failed: {0}")]
    Bitcode(#[from] bitcode::Error),
    #[error("LZ4 decompression failed: {0}")]
    Lz4(#[from] DecompressError),
    #[error("reading serialized bytes failed: {0}")]
    Read(#[from] io::Error),
    #[error(transparent)]
    Lz4Size(#[from] Lz4SizeError),
    #[error("lz4 payload too short: {len} bytes")]
    Lz4PayloadTooShort { len: usize },
    #[error("lz4 decompressed size mismatch: got {actual}, expected {expected}")]
    Lz4DecompressedSizeMismatch { actual: usize, expected: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Lz4SizeError {
    #[error("lz4 uncompressed size {size} exceeds 32-bit header capacity")]
    HeaderCapacity { size: usize },
    #[error("lz4 uncompressed size {size} exceeds limit {limit}")]
    Limit { size: usize, limit: usize },
}

#[expect(
    clippy::map_err_ignore,
    reason = "a `TryFromIntError` says only that the value does not fit, which the error it becomes states"
)]
fn checked_lz4_uncompressed_size(uncompressed_size: usize) -> Result<u32, Lz4SizeError> {
    let header_size =
        u32::try_from(uncompressed_size).map_err(|_| Lz4SizeError::HeaderCapacity {
            size: uncompressed_size,
        })?;
    if uncompressed_size > LZ4_MAX_UNCOMPRESSED_SIZE {
        return Err(Lz4SizeError::Limit {
            size: uncompressed_size,
            limit: LZ4_MAX_UNCOMPRESSED_SIZE,
        });
    }
    Ok(header_size)
}

fn lz4_payload(serialized: &[u8]) -> Result<Lz4Payload<'_>, DeserializeError> {
    if serialized.len() < LZ4_HEADER_LEN {
        return Err(DeserializeError::Lz4PayloadTooShort {
            len: serialized.len(),
        });
    }

    let uncompressed_size =
        u32::from_le_bytes(serialized[..LZ4_HEADER_LEN].try_into().unwrap()) as usize;
    checked_lz4_uncompressed_size(uncompressed_size)?;
    Ok(Lz4Payload {
        compressed: &serialized[LZ4_HEADER_LEN..],
        uncompressed_size,
    })
}

fn check_lz4_decompressed_size(actual: usize, expected: usize) -> Result<(), DeserializeError> {
    if actual != expected {
        return Err(DeserializeError::Lz4DecompressedSizeMismatch { actual, expected });
    }
    Ok(())
}

pub fn serialize<T: Serialize>(value: &T, format: SerdeFormat) -> Result<Vec<u8>, SerializeError> {
    let mut buffer = Vec::new();
    let mut temp_buffer = Vec::new();
    serialize_into(value, format, &mut buffer, &mut temp_buffer)?;
    Ok(buffer)
}

/// `temp_buffer` is reusable scratch the caller threads across calls to avoid
/// per-call allocation in hot paths (e.g. undo-step coalescing). Pass a
/// long-lived `Vec` you reuse; it's cleared on entry. (Bitcode doesn't touch
/// it on serialize; the RON and LZ4 arms use it.)
pub fn serialize_into<T: Serialize, W: Write>(
    value: T,
    format: SerdeFormat,
    writer: &mut W,
    temp_buffer: &mut Vec<u8>,
) -> Result<(), SerializeError> {
    temp_buffer.clear();

    match format {
        SerdeFormat::Ron => {
            let config = PrettyConfig::default();
            ser::to_writer_pretty(Utf8Writer(&mut *temp_buffer), &value, config)?;
            writer.write_all(temp_buffer)?;
        }
        SerdeFormat::Bitcode => {
            let encoded = bitcode::serialize(&value)?;
            writer.write_all(&encoded)?;
        }
        SerdeFormat::Lz4 => {
            ser::to_writer(Utf8Writer(&mut *temp_buffer), &value)?;

            let uncompressed_size = temp_buffer.len();
            let header_size = checked_lz4_uncompressed_size(uncompressed_size)?;
            writer.write_all(&header_size.to_le_bytes())?;

            let max_compressed_size = block::get_maximum_output_size(uncompressed_size);
            temp_buffer.resize(uncompressed_size + max_compressed_size, 0);

            let (input, output) = temp_buffer.split_at_mut(uncompressed_size);
            let compressed_len = lz4_flex::compress_into(input, output)?;
            writer.write_all(&output[..compressed_len])?;
        }
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
        SerdeFormat::Lz4 => {
            let payload = lz4_payload(serialized)?;
            let mut decompressed = vec![0; payload.uncompressed_size];
            let decompressed_len =
                lz4_flex::decompress_into(payload.compressed, &mut decompressed)?;
            check_lz4_decompressed_size(decompressed_len, payload.uncompressed_size)?;
            Ok(de::from_bytes(&decompressed)?)
        }
    }
}

/// `temp_buffer` is reusable scratch (read buffer / LZ4 work area) the caller
/// threads across calls to avoid per-call allocation in hot paths. Cleared on entry.
pub fn deserialize_from<T: DeserializeOwned, R: Read>(
    reader: &mut R,
    format: SerdeFormat,
    temp_buffer: &mut Vec<u8>,
) -> Result<T, DeserializeError> {
    temp_buffer.clear();

    match format {
        SerdeFormat::Ron => Ok(de::from_reader(reader)?),
        SerdeFormat::Bitcode => {
            reader.read_to_end(temp_buffer)?;
            Ok(bitcode::deserialize(temp_buffer.as_slice())?)
        }
        SerdeFormat::Lz4 => {
            reader.read_to_end(temp_buffer)?;
            let uncompressed_size = lz4_payload(temp_buffer)?.uncompressed_size;
            let compressed_len = temp_buffer.len() - LZ4_HEADER_LEN;
            temp_buffer.resize(LZ4_HEADER_LEN + compressed_len + uncompressed_size, 0);

            let (compressed_part, decompressed_part) =
                temp_buffer.split_at_mut(LZ4_HEADER_LEN + compressed_len);
            let compressed = &compressed_part[LZ4_HEADER_LEN..];

            let decompressed_len = lz4_flex::decompress_into(compressed, decompressed_part)?;
            check_lz4_decompressed_size(decompressed_len, uncompressed_size)?;

            Ok(de::from_bytes(decompressed_part)?)
        }
    }
}

#[cfg(test)]
mod tests;
