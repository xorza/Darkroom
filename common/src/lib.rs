use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;

use crate::normalize_string::NormalizeString;

#[macro_use]
pub mod macros;
pub mod normalize_string;
pub mod output_stream;
pub mod scoped_ref;
pub mod serde_lua;
pub mod toggle;
pub mod yaml_format;

pub const EPSILON: f64 = 1e-6;

#[derive(Debug, thiserror::Error)]
pub enum FileExtensionError {
    #[error("Failed to get file extension")]
    MissingFileExtension,
    #[error("Unsupported file extension for file: {0}")]
    UnsupportedFileExtension(String),
}

pub type FileFormatResult<T> = Result<T, FileExtensionError>;

pub fn get_file_extension(filename: &str) -> Option<&str> {
    Path::new(filename)
        .extension()
        .and_then(|os_str| os_str.to_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Yaml,
    Json,
    Lua,
}

impl FileFormat {
    pub fn from_file_name(file_name: &str) -> FileFormatResult<Self> {
        let extension = get_file_extension(file_name)
            .map(|ext| ext.to_ascii_lowercase())
            .ok_or(FileExtensionError::MissingFileExtension)?;

        match extension.as_str() {
            "yaml" | "yml" => Ok(Self::Yaml),
            "json" => Ok(Self::Json),
            "lua" => Ok(Self::Lua),
            _ => Err(FileExtensionError::UnsupportedFileExtension(
                file_name.to_string(),
            )),
        }
    }
}

pub fn is_debug() -> bool {
    cfg!(debug_assertions)
}

pub fn serialize<T: Serialize>(value: &T, format: FileFormat) -> String {
    match format {
        FileFormat::Yaml => serde_yml::to_string(value).unwrap(),
        FileFormat::Json => serde_json::to_string_pretty(value).unwrap(),
        FileFormat::Lua => serde_lua::to_string(value).unwrap(),
    }
    .normalize()
}

pub fn deserialize<T: DeserializeOwned>(serialized: &str, format: FileFormat) -> anyhow::Result<T> {
    match format {
        FileFormat::Yaml => serde_yml::from_str(serialized).map_err(anyhow::Error::from),
        FileFormat::Json => serde_json::from_str(serialized).map_err(anyhow::Error::from),
        FileFormat::Lua => serde_lua::from_str(serialized).map_err(anyhow::Error::from),
    }
}
