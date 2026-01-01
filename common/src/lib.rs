use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::normalize_string::NormalizeString;
use crate::serde_lua::SerdeLuaError;

#[macro_use]
pub mod macros;
pub mod key_index_vec;
pub mod normalize_string;
pub mod output_stream;
pub mod scoped_ref;
pub mod serde_lua;
pub mod toggle;
pub mod yaml_format;

pub const EPSILON: f64 = 1e-6;

pub type ArcMutex<T> = Arc<Mutex<T>>;

#[derive(Clone, Debug)]
pub struct Shared<T> {
    inner: Arc<Mutex<T>>,
}

impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(value)),
        }
    }

    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, T> {
        self.inner.lock().await
    }

    pub async fn lock_owned(&self) -> tokio::sync::OwnedMutexGuard<T> {
        self.inner.clone().lock_owned().await
    }

    pub fn try_lock(&self) -> Result<tokio::sync::MutexGuard<'_, T>, tokio::sync::TryLockError> {
        self.inner.try_lock()
    }

    pub fn get_mut(&mut self) -> &mut T {
        Arc::get_mut(&mut self.inner)
            .expect("Shared::get_mut requires unique ownership of the inner Arc")
            .get_mut()
    }

    pub fn arc(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.inner)
    }
}

impl<T> std::ops::Deref for Shared<T> {
    type Target = Arc<Mutex<T>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> From<Arc<Mutex<T>>> for Shared<T> {
    fn from(inner: Arc<Mutex<T>>) -> Self {
        Self { inner }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FileExtensionError {
    #[error("Failed to get file extension")]
    MissingFileExtension,
    #[error("Unsupported file extension for file: {0}")]
    UnsupportedFileExtension(String),
}

pub type FileFormatResult<T> = Result<T, FileExtensionError>;

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
