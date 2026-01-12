use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_format::FileFormat;
use crate::normalize_string::NormalizeString;
use crate::serde_lua;

pub fn is_false(value: &bool) -> bool {
    !*value
}

pub type SerdeFormatResult<T> = anyhow::Result<T>;

pub fn serialize<T: Serialize>(value: &T, format: FileFormat) -> Vec<u8> {
    match format {
        FileFormat::Yaml => serde_yml::to_string(value)
            .unwrap()
            .normalize()
            .into_bytes(),
        FileFormat::Json => serde_json::to_string_pretty(value)
            .unwrap()
            .normalize()
            .into_bytes(),
        FileFormat::Lua => serde_lua::to_string(value)
            .unwrap()
            .normalize()
            .into_bytes(),
        FileFormat::Bin => {
            bincode::serde::encode_to_vec(value, bincode::config::standard()).unwrap()
        }
    }
}

pub fn deserialize<T: DeserializeOwned>(
    serialized: &[u8],
    format: FileFormat,
) -> SerdeFormatResult<T> {
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
        FileFormat::Bin => {
            let (decoded, read) =
                bincode::serde::decode_from_slice(serialized, bincode::config::standard())?;
            if read != serialized.len() {
                anyhow::bail!("binary payload should be fully consumed");
            }
            Ok(decoded)
        }
    }
}
