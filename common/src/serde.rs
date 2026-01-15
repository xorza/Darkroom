use std::io::Write;

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
    value: &T,
    format: FileFormat,
    writer: &mut W,
    temp_buffer: &mut Vec<u8>,
) {
    temp_buffer.clear();

    match format {
        FileFormat::Yaml => {
            let s = serde_yml::to_string(value).unwrap().normalize();
            writer.write_all(s.as_bytes()).unwrap();
        }
        FileFormat::Json => {
            let s = serde_json::to_string_pretty(value).unwrap().normalize();
            writer.write_all(s.as_bytes()).unwrap();
        }
        FileFormat::Lua => {
            let s = serde_lua::to_string(value).unwrap().normalize();
            writer.write_all(s.as_bytes()).unwrap();
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
            let s = toml::to_string(value).unwrap().normalize();
            writer.write_all(s.as_bytes()).unwrap();
        }
    }
}

pub fn deserialize<T: DeserializeOwned>(serialized: &[u8], format: FileFormat) -> Result<T> {
    match format {
        FileFormat::Yaml => {
            let text = std::str::from_utf8(serialized)?;
            Ok(serde_yml::from_str(text)?)
        }
        FileFormat::Json => {
            let text = std::str::from_utf8(serialized)?;
            Ok(serde_json::from_str(text)?)
        }
        FileFormat::Lua => {
            let text = std::str::from_utf8(serialized)?;
            Ok(serde_lua::from_str(text)?)
        }
        FileFormat::Toml => {
            let text = std::str::from_utf8(serialized)?;
            Ok(toml::from_str(text)?)
        }
        FileFormat::Bin => {
            let decompressed = lz4_flex::decompress_size_prepended(serialized)?;
            let (decoded, read) =
                bincode::serde::decode_from_slice(&decompressed, bincode::config::standard())?;
            if read != decompressed.len() {
                anyhow::bail!("binary payload should be fully consumed");
            }
            Ok(decoded)
        }
    }
}
