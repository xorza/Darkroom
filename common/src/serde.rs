use std::io::{Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_format::SerdeFormat;

const LZ4_HEADER_LEN: usize = size_of::<u32>();
const LZ4_MAX_UNCOMPRESSED_SIZE: usize = 1 << 30;

#[derive(Debug)]
struct Lz4Payload<'a> {
    compressed: &'a [u8],
    uncompressed_size: usize,
}

fn normalize(mut input: String) -> String {
    if !input.as_bytes().contains(&b'\r') {
        if !input.ends_with('\n') {
            input.push('\n');
        }
        return input;
    }

    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'\r' {
            index += 1;
            continue;
        }

        output.push_str(&input[last..index]);
        if index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
            index += 1;
        }
        output.push('\n');
        index += 1;
        last = index;
    }

    output.push_str(&input[last..]);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

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
            let s = normalize(toml::to_string(&value)?);
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
    match format {
        SerdeFormat::Json => Ok(serde_json::from_slice(serialized)?),
        SerdeFormat::Toml => Ok(toml::from_slice(serialized)?),
        SerdeFormat::Bitcode => Ok(bitcode::deserialize(serialized)?),
        SerdeFormat::Lz4 => {
            let payload = lz4_payload(serialized)?;
            let mut decompressed = vec![0; payload.uncompressed_size];
            let decompressed_len =
                lz4_flex::decompress_into(payload.compressed, &mut decompressed)?;
            check_lz4_decompressed_size(decompressed_len, payload.uncompressed_size)?;
            Ok(serde_json::from_slice(&decompressed)?)
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
            let uncompressed_size = lz4_payload(temp_buffer)?.uncompressed_size;
            let compressed_len = temp_buffer.len() - LZ4_HEADER_LEN;
            temp_buffer.resize(LZ4_HEADER_LEN + compressed_len + uncompressed_size, 0);

            let (compressed_part, decompressed_part) =
                temp_buffer.split_at_mut(LZ4_HEADER_LEN + compressed_len);
            let compressed = &compressed_part[LZ4_HEADER_LEN..];

            let decompressed_len = lz4_flex::decompress_into(compressed, decompressed_part)?;
            check_lz4_decompressed_size(decompressed_len, uncompressed_size)?;

            Ok(serde_json::from_slice(decompressed_part)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

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

    #[derive(Debug, PartialEq, Serialize, serde::Deserialize)]
    struct TestValue {
        label: String,
        count: u32,
    }

    #[test]
    fn slice_and_reader_dispatch_match_for_every_format() {
        let value = TestValue {
            label: "payload".to_string(),
            count: 42,
        };

        for format in [
            SerdeFormat::Json,
            SerdeFormat::Toml,
            SerdeFormat::Bitcode,
            SerdeFormat::Lz4,
        ] {
            let bytes = serialize(&value, format).unwrap();
            let direct: TestValue = deserialize(&bytes, format).unwrap();
            let streamed: TestValue =
                deserialize_from(&mut Cursor::new(&bytes), format, &mut Vec::new()).unwrap();
            assert_eq!(direct, value, "direct {format:?}");
            assert_eq!(streamed, value, "reader {format:?}");

            let mut with_trailing_data = bytes;
            with_trailing_data.extend_from_slice(match format {
                SerdeFormat::Json => b"x",
                SerdeFormat::Toml => b"\n=",
                SerdeFormat::Bitcode | SerdeFormat::Lz4 => &[0xff],
            });
            let direct: Result<TestValue, _> = deserialize(&with_trailing_data, format);
            let streamed: Result<TestValue, _> = deserialize_from(
                &mut Cursor::new(&with_trailing_data),
                format,
                &mut Vec::new(),
            );
            match (direct, streamed) {
                (Ok(direct), Ok(streamed)) => {
                    assert_eq!(direct, streamed, "trailing data for {format:?}");
                }
                (Err(_), Err(_)) => {}
                (direct, streamed) => panic!(
                    "slice and reader trailing-data behavior differs for {format:?}: \
                     direct={}, reader={}",
                    direct.is_ok(),
                    streamed.is_ok()
                ),
            }
        }
    }

    #[test]
    fn toml_line_endings_and_final_newline_are_normalized() {
        for (input, expected) in [
            ("", "\n"),
            ("\n", "\n"),
            ("hello", "hello\n"),
            ("a\nb\n", "a\nb\n"),
            ("a\r\nb", "a\nb\n"),
            ("a\rb\r", "a\nb\n"),
            ("a\nb\r\nc\rd", "a\nb\nc\nd\n"),
            ("\r\n\r\r\n", "\n\n\n"),
            ("héllo 🎉\r\n你好", "héllo 🎉\n你好\n"),
        ] {
            assert_eq!(normalize(input.to_string()), expected, "input {input:?}");
        }
    }

    #[test]
    fn toml_normalization_reuses_allocations_without_cr() {
        for input in ["already normalized\n", "needs newline"] {
            let mut value = String::with_capacity(64);
            value.push_str(input);
            let pointer = value.as_ptr();
            let capacity = value.capacity();

            let normalized = normalize(value);
            assert_eq!(normalized.as_ptr(), pointer);
            assert_eq!(normalized.capacity(), capacity);
        }
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
