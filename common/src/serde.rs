use std::io::{Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_format::FileFormat;
use crate::normalize_string::NormalizeString;
use crate::serde_lua;

pub fn is_false(value: &bool) -> bool {
    !*value
}

pub type Result<T> = anyhow::Result<T>;

pub fn serialize<T: Serialize>(value: &T, format: FileFormat) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut temp_buffer = Vec::new();
    serialize_into(value, format, &mut buffer, &mut temp_buffer);
    buffer
}

pub fn serialize_into<T: Serialize, W: Write>(
    value: T,
    format: FileFormat,
    writer: &mut W,
    temp_buffer: &mut Vec<u8>,
) {
    temp_buffer.clear();

    match format {
        FileFormat::Yaml => {
            serde_yml::to_writer(writer, &value).unwrap();
        }
        FileFormat::Json => {
            serde_json::to_writer_pretty(writer, &value).unwrap();
        }
        FileFormat::Lua => {
            serde_lua::to_writer(writer, &value).unwrap();
        }
        FileFormat::Bin => {
            bincode::serde::encode_into_std_write(value, temp_buffer, bincode::config::standard())
                .unwrap();

            // Prepend uncompressed size (4 bytes, little-endian)
            let uncompressed_size = temp_buffer.len();
            writer
                .write_all(&(uncompressed_size as u32).to_le_bytes())
                .unwrap();

            let max_compressed_size = lz4_flex::block::get_maximum_output_size(uncompressed_size);
            temp_buffer.resize(uncompressed_size + max_compressed_size, 0);

            let (input, output) = temp_buffer.split_at_mut(uncompressed_size);

            let compressed_len = lz4_flex::compress_into(input, output).unwrap();
            writer.write_all(&output[..compressed_len]).unwrap();
        }
        FileFormat::Toml => {
            let s = toml::to_string(&value).unwrap().normalize();
            writer.write_all(s.as_bytes()).unwrap();
        }
    }
}

pub fn deserialize<T: DeserializeOwned>(serialized: &[u8], format: FileFormat) -> Result<T> {
    let mut temp_buffer = Vec::new();
    deserialize_from(
        &mut std::io::Cursor::new(serialized),
        format,
        &mut temp_buffer,
    )
}

pub fn deserialize_from<T: DeserializeOwned, R: Read>(
    reader: &mut R,
    format: FileFormat,
    temp_buffer: &mut Vec<u8>,
) -> Result<T> {
    temp_buffer.clear();

    match format {
        FileFormat::Yaml => Ok(serde_yml::from_reader(reader)?),
        FileFormat::Json => Ok(serde_json::from_reader(reader)?),
        FileFormat::Lua => Ok(serde_lua::from_reader(reader)?),
        FileFormat::Toml => {
            reader.read_to_end(temp_buffer)?;
            Ok(toml::from_slice(temp_buffer.as_slice())?)
        }
        FileFormat::Bin => {
            reader.read_to_end(temp_buffer)?;
            let uncompressed_size =
                u32::from_le_bytes(temp_buffer[0..4].try_into().unwrap()) as usize;
            let compressed_start = 4;
            let compressed_len = temp_buffer.len() - compressed_start;

            // Reserve space for decompressed data at the end
            temp_buffer.resize(compressed_start + compressed_len + uncompressed_size, 0);

            let (compressed_part, decompressed_part) =
                temp_buffer.split_at_mut(compressed_start + compressed_len);
            let compressed = &compressed_part[compressed_start..];

            let decompressed_len = lz4_flex::decompress_into(compressed, decompressed_part)?;
            assert_eq!(
                decompressed_len, uncompressed_size,
                "decompressed size mismatch"
            );

            let (decoded, read) =
                bincode::serde::decode_from_slice(decompressed_part, bincode::config::standard())?;
            assert_eq!(
                read, uncompressed_size,
                "binary payload should be fully consumed"
            );

            Ok(decoded)
        }
    }
}
