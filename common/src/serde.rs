use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::file_format::FileFormat;
use crate::normalize_string::NormalizeString;
use crate::serde_lua;
use crate::serde_lua::SerdeLuaError;

pub fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, thiserror::Error)]
pub enum SerdeFormatError {
    #[error("YAML serialization failed")]
    Yaml(#[from] serde_yml::Error),
    #[error("JSON serialization failed")]
    Json(#[from] serde_json::Error),
    #[error("Lua serialization failed")]
    Lua(#[from] SerdeLuaError),
}

pub type SerdeFormatResult<T> = Result<T, SerdeFormatError>;

pub fn serialize<T: Serialize>(value: &T, format: FileFormat) -> String {
    match format {
        FileFormat::Yaml => serde_yml::to_string(value).unwrap(),
        FileFormat::Json => serde_json::to_string_pretty(value).unwrap(),
        FileFormat::Lua => serde_lua::to_string(value).unwrap(),
    }
    .normalize()
}

pub fn deserialize<T: DeserializeOwned>(
    serialized: &str,
    format: FileFormat,
) -> SerdeFormatResult<T> {
    match format {
        FileFormat::Yaml => Ok(serde_yml::from_str(serialized)?),
        FileFormat::Json => Ok(serde_json::from_str(serialized)?),
        FileFormat::Lua => Ok(serde_lua::from_str(serialized)?),
    }
}
