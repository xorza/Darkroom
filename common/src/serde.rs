use std::io::{Cursor, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_format::SerdeFormat;
use crate::normalize_string::NormalizeString;

const LZ4_HEADER_LEN: usize = size_of::<u32>();
const LZ4_MAX_UNCOMPRESSED_SIZE: usize = 1 << 30;

#[derive(Debug, thiserror::Error)]
pub enum SerializeError {
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Bitcode serialization failed: {0}")]
    Bitcode(#[from] bitcode::Error),
    #[error("TOML serialization failed: {0}")]
    Toml(#[from] toml::ser::Error),
    #[error("LZ4 compression failed: {0}")]
    Lz4(#[from] lz4_flex::block::CompressError),
    #[error("writing serialized bytes failed: {0}")]
    Write(#[from] std::io::Error),
    #[error(transparent)]
    Lz4Size(#[from] Lz4SizeError),
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    #[error("JSON deserialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Bitcode deserialization failed: {0}")]
    Bitcode(#[from] bitcode::Error),
    #[error("TOML deserialization failed: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("LZ4 decompression failed: {0}")]
    Lz4(#[from] lz4_flex::block::DecompressError),
    #[error("reading serialized bytes failed: {0}")]
    Read(#[from] std::io::Error),
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

pub fn serialize<T: Serialize>(value: &T, format: SerdeFormat) -> Result<Vec<u8>, SerializeError> {
    let mut buffer = Vec::new();
    let mut temp_buffer = Vec::new();
    serialize_into(value, format, &mut buffer, &mut temp_buffer)?;
    Ok(buffer)
}

/// `temp_buffer` is reusable scratch the caller threads across calls to avoid
/// per-call allocation in hot paths (e.g. undo-step coalescing). Pass a
/// long-lived `Vec` you reuse; it's cleared on entry. (Bitcode/JSON don't touch
/// it on serialize; the LZ4 arm uses it.)
pub fn serialize_into<T: Serialize, W: Write>(
    value: T,
    format: SerdeFormat,
    writer: &mut W,
    temp_buffer: &mut Vec<u8>,
) -> Result<(), SerializeError> {
    temp_buffer.clear();

    match format {
        SerdeFormat::Json => serde_json::to_writer_pretty(writer, &value)?,
        SerdeFormat::Bitcode => {
            let encoded = bitcode::serialize(&value)?;
            writer.write_all(&encoded)?;
        }
        SerdeFormat::Toml => {
            let s = toml::to_string(&value)?.normalize();
            writer.write_all(s.as_bytes())?;
        }
        SerdeFormat::Lz4 => {
            serde_json::to_writer(&mut *temp_buffer, &value)?;

            let uncompressed_size = temp_buffer.len();
            let header_size = checked_lz4_uncompressed_size(uncompressed_size)?;
            writer.write_all(&header_size.to_le_bytes())?;

            let max_compressed_size = lz4_flex::block::get_maximum_output_size(uncompressed_size);
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
    let mut temp_buffer = Vec::new();
    deserialize_from(&mut Cursor::new(serialized), format, &mut temp_buffer)
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
        SerdeFormat::Json => Ok(serde_json::from_reader(reader)?),
        SerdeFormat::Toml => {
            reader.read_to_end(temp_buffer)?;
            Ok(toml::from_slice(temp_buffer.as_slice())?)
        }
        SerdeFormat::Bitcode => {
            reader.read_to_end(temp_buffer)?;
            Ok(bitcode::deserialize(temp_buffer.as_slice())?)
        }
        SerdeFormat::Lz4 => {
            reader.read_to_end(temp_buffer)?;

            // Trust-boundary parser: the header length is attacker-controlled, so
            // every step is bounds-/range-checked and returns `Err` rather than panicking.
            if temp_buffer.len() < LZ4_HEADER_LEN {
                return Err(DeserializeError::Lz4PayloadTooShort {
                    len: temp_buffer.len(),
                });
            }

            let uncompressed_size =
                u32::from_le_bytes(temp_buffer[0..LZ4_HEADER_LEN].try_into().unwrap()) as usize;
            checked_lz4_uncompressed_size(uncompressed_size)?;

            let compressed_len = temp_buffer.len() - LZ4_HEADER_LEN;
            temp_buffer.resize(LZ4_HEADER_LEN + compressed_len + uncompressed_size, 0);

            let (compressed_part, decompressed_part) =
                temp_buffer.split_at_mut(LZ4_HEADER_LEN + compressed_len);
            let compressed = &compressed_part[LZ4_HEADER_LEN..];

            let decompressed_len = lz4_flex::decompress_into(compressed, decompressed_part)?;
            if decompressed_len != uncompressed_size {
                return Err(DeserializeError::Lz4DecompressedSizeMismatch {
                    actual: decompressed_len,
                    expected: uncompressed_size,
                });
            }

            Ok(serde_json::from_slice(decompressed_part)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn backend_failures_keep_their_typed_variant() {
        assert!(matches!(
            deserialize::<i64>(b"{", SerdeFormat::Json).unwrap_err(),
            DeserializeError::Json(_)
        ));
        assert!(matches!(
            deserialize::<i64>(b"x =", SerdeFormat::Toml).unwrap_err(),
            DeserializeError::Toml(_)
        ));
        assert!(matches!(
            deserialize::<i64>(&[], SerdeFormat::Bitcode).unwrap_err(),
            DeserializeError::Bitcode(_)
        ));

        let err = serialize_into(
            1i64,
            SerdeFormat::Bitcode,
            &mut FailingWriter,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SerializeError::Write(error) if error.kind() == std::io::ErrorKind::BrokenPipe
        ));
    }

    #[test]
    fn lz4_round_trips_json_payload() {
        let value: Vec<i64> = vec![1, 2, 3, 1000, -42];
        let bytes = serialize(&value, SerdeFormat::Lz4).unwrap();
        let expected_size = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let payload = lz4_flex::block::decompress(&bytes[4..], expected_size).unwrap();
        assert_eq!(payload, br#"[1,2,3,1000,-42]"#);
        let back: Vec<i64> = deserialize(&bytes, SerdeFormat::Lz4).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn lz4_encode_size_boundaries_are_checked() {
        assert_eq!(
            checked_lz4_uncompressed_size(LZ4_MAX_UNCOMPRESSED_SIZE).unwrap(),
            LZ4_MAX_UNCOMPRESSED_SIZE as u32
        );

        let over_limit = LZ4_MAX_UNCOMPRESSED_SIZE + 1;
        let err = checked_lz4_uncompressed_size(over_limit).unwrap_err();
        assert_eq!(
            err,
            Lz4SizeError::Limit {
                size: over_limit,
                limit: LZ4_MAX_UNCOMPRESSED_SIZE,
            }
        );

        if let Ok(over_header) = usize::try_from(u64::from(u32::MAX) + 1) {
            let err = checked_lz4_uncompressed_size(over_header).unwrap_err();
            assert_eq!(err, Lz4SizeError::HeaderCapacity { size: over_header });
        }
    }

    #[test]
    fn lz4_payload_shorter_than_header_errors() {
        for input in [&[][..], &[0][..], &[0, 1][..], &[0, 1, 2][..]] {
            let err = deserialize::<i64>(input, SerdeFormat::Lz4).unwrap_err();
            assert!(
                matches!(
                    err,
                    DeserializeError::Lz4PayloadTooShort { len } if len == input.len()
                ),
                "unexpected error for {}-byte payload: {err}",
                input.len(),
            );
        }
    }

    #[test]
    fn lz4_oversized_length_prefix_is_rejected() {
        let oversized = 0x7FFF_FFFFu32 as usize;
        let mut input = (oversized as u32).to_le_bytes().to_vec();
        input.extend_from_slice(&[1, 2, 3]);
        let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
        assert!(matches!(
            err,
            DeserializeError::Lz4Size(Lz4SizeError::Limit {
                size,
                limit: LZ4_MAX_UNCOMPRESSED_SIZE,
            }) if size == oversized
        ));

        let just_over = LZ4_MAX_UNCOMPRESSED_SIZE + 1;
        let input = (just_over as u32).to_le_bytes();
        let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
        assert!(matches!(
            err,
            DeserializeError::Lz4Size(Lz4SizeError::Limit {
                size,
                limit: LZ4_MAX_UNCOMPRESSED_SIZE,
            }) if size == just_over
        ));
    }

    #[test]
    fn lz4_corrupt_and_mismatched_bodies_return_typed_errors() {
        let mut input = 8u32.to_le_bytes().to_vec();
        input.extend_from_slice(&[0xFF; 16]);
        let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
        assert!(matches!(err, DeserializeError::Lz4(_)));

        let json = b"1";
        let expected = json.len() + 1;
        let mut input = (expected as u32).to_le_bytes().to_vec();
        input.extend_from_slice(&lz4_flex::block::compress(json));
        let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
        assert!(matches!(
            err,
            DeserializeError::Lz4DecompressedSizeMismatch {
                actual,
                expected: error_expected,
            } if actual == json.len() && error_expected == expected
        ));
    }
}
