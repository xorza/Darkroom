use std::io::{Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_format::SerdeFormat;
use crate::normalize_string::NormalizeString;
use crate::serde_scn;

pub fn is_false(value: &bool) -> bool {
    !*value
}

pub type Result<T> = anyhow::Result<T>;

pub fn serialize<T: Serialize>(value: &T, format: SerdeFormat) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut temp_buffer = Vec::new();
    serialize_into(value, format, &mut buffer, &mut temp_buffer);
    buffer
}

pub fn serialize_into<T: Serialize, W: Write>(
    value: T,
    format: SerdeFormat,
    writer: &mut W,
    temp_buffer: &mut Vec<u8>,
) {
    temp_buffer.clear();

    match format {
        SerdeFormat::Json => {
            serde_json::to_writer_pretty(writer, &value).unwrap();
        }
        SerdeFormat::Bitcode => {
            let encoded = bitcode::serialize(&value).unwrap();
            writer.write_all(&encoded).unwrap();
        }
        SerdeFormat::Toml => {
            let s = toml::to_string(&value).unwrap().normalize();
            writer.write_all(s.as_bytes()).unwrap();
        }
        SerdeFormat::Lz4 => {
            let _ = (writer, value, temp_buffer);
            unimplemented!("Lz4 serialization pending a non-Lua inner format");
        }
        SerdeFormat::ScnText => {
            serde_scn::to_writer(writer, &value).unwrap();
        }
    }
}

pub fn deserialize<T: DeserializeOwned>(serialized: &[u8], format: SerdeFormat) -> Result<T> {
    let mut temp_buffer = Vec::new();
    deserialize_from(
        &mut std::io::Cursor::new(serialized),
        format,
        &mut temp_buffer,
    )
}

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
            let _ = (reader, temp_buffer);
            unimplemented!("Lz4 deserialization pending a non-Lua inner format");
        }
        SerdeFormat::ScnText => Ok(serde_scn::from_reader(reader)?),
    }
}
