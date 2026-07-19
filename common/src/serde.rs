use std::io::{Cursor, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_format::SerdeFormat;
use crate::normalize_string::NormalizeString;

pub type Result<T> = anyhow::Result<T>;

const LZ4_HEADER_LEN: usize = size_of::<u32>();
const LZ4_MAX_UNCOMPRESSED_SIZE: usize = 1 << 30;

fn checked_lz4_uncompressed_size(uncompressed_size: usize) -> Result<u32> {
    let header_size = u32::try_from(uncompressed_size).map_err(|_| {
        anyhow::anyhow!("lz4 uncompressed size {uncompressed_size} exceeds 32-bit header capacity")
    })?;
    if uncompressed_size > LZ4_MAX_UNCOMPRESSED_SIZE {
        anyhow::bail!(
            "lz4 uncompressed size {uncompressed_size} exceeds limit \
             {LZ4_MAX_UNCOMPRESSED_SIZE}"
        );
    }
    Ok(header_size)
}

pub fn serialize<T: Serialize>(value: &T, format: SerdeFormat) -> Result<Vec<u8>> {
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
) -> Result<()> {
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

pub fn deserialize<T: DeserializeOwned>(serialized: &[u8], format: SerdeFormat) -> Result<T> {
    let mut temp_buffer = Vec::new();
    deserialize_from(&mut Cursor::new(serialized), format, &mut temp_buffer)
}

/// `temp_buffer` is reusable scratch (read buffer / LZ4 work area) the caller
/// threads across calls to avoid per-call allocation in hot paths. Cleared on entry.
pub fn deserialize_from<T: DeserializeOwned, R: Read>(
    reader: &mut R,
    format: SerdeFormat,
    temp_buffer: &mut Vec<u8>,
) -> Result<T> {
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
                anyhow::bail!("lz4 payload too short: {} bytes", temp_buffer.len());
            }

            let uncompressed_size =
                u32::from_le_bytes(temp_buffer[0..LZ4_HEADER_LEN].try_into().unwrap()) as usize;
            if uncompressed_size > LZ4_MAX_UNCOMPRESSED_SIZE {
                anyhow::bail!(
                    "lz4 uncompressed size {uncompressed_size} exceeds limit \
                     {LZ4_MAX_UNCOMPRESSED_SIZE}"
                );
            }

            let compressed_len = temp_buffer.len() - LZ4_HEADER_LEN;
            temp_buffer.resize(LZ4_HEADER_LEN + compressed_len + uncompressed_size, 0);

            let (compressed_part, decompressed_part) =
                temp_buffer.split_at_mut(LZ4_HEADER_LEN + compressed_len);
            let compressed = &compressed_part[LZ4_HEADER_LEN..];

            let decompressed_len = lz4_flex::decompress_into(compressed, decompressed_part)?;
            if decompressed_len != uncompressed_size {
                anyhow::bail!(
                    "lz4 decompressed size mismatch: got {decompressed_len}, expected {uncompressed_size}"
                );
            }

            Ok(serde_json::from_slice(decompressed_part)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            err.to_string(),
            format!(
                "lz4 uncompressed size {over_limit} exceeds limit \
                 {LZ4_MAX_UNCOMPRESSED_SIZE}"
            )
        );

        if let Ok(over_header) = usize::try_from(u64::from(u32::MAX) + 1) {
            let err = checked_lz4_uncompressed_size(over_header).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("lz4 uncompressed size {over_header} exceeds 32-bit header capacity")
            );
        }
    }

    #[test]
    fn lz4_payload_shorter_than_header_errors() {
        // < 4 bytes: the length prefix isn't even present.
        for input in [&[][..], &[0][..], &[0, 1][..], &[0, 1, 2][..]] {
            let err = deserialize::<i64>(input, SerdeFormat::Lz4);
            assert!(
                err.is_err(),
                "{}-byte payload must Err, not panic",
                input.len()
            );
        }
    }

    #[test]
    fn lz4_oversized_length_prefix_is_rejected() {
        // Header claims ~2 GiB uncompressed — above the 1 GiB ceiling — so the
        // decoder must reject it instead of attempting the allocation.
        let mut input = 0x7FFF_FFFFu32.to_le_bytes().to_vec();
        input.extend_from_slice(&[1, 2, 3]);
        let err = deserialize::<i64>(&input, SerdeFormat::Lz4);
        assert!(err.is_err());
        // Exactly at the ceiling boundary is also rejected (> limit is the check,
        // and (1<<30)+1 exceeds it); a 4-byte-only buffer has no compressed body.
        let just_over = ((1u32 << 30) + 1).to_le_bytes().to_vec();
        assert!(deserialize::<i64>(&just_over, SerdeFormat::Lz4).is_err());
    }

    #[test]
    fn lz4_corrupt_body_errors_without_panicking() {
        // Plausible header (size 8) followed by garbage that isn't a valid LZ4
        // block: decompress fails or the size mismatches — either way, an Err.
        let mut input = 8u32.to_le_bytes().to_vec();
        input.extend_from_slice(&[0xFF; 16]);
        let err = deserialize::<i64>(&input, SerdeFormat::Lz4);
        assert!(err.is_err());
    }
}
