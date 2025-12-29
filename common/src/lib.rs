use std::path::Path;

#[macro_use]
pub mod macros;
pub mod normalize_string;
pub mod output_stream;
pub mod scoped_ref;
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

pub type CommonResult<T> = Result<T, FileExtensionError>;

pub fn get_file_extension(filename: &str) -> Option<&str> {
    Path::new(filename)
        .extension()
        .and_then(|os_str| os_str.to_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Yaml,
    Json,
}

impl FileFormat {
    pub fn from_file_name(file_name: &str) -> CommonResult<Self> {
        let extension = get_file_extension(file_name)
            .map(|ext| ext.to_ascii_lowercase())
            .ok_or(FileExtensionError::MissingFileExtension)?;

        match extension.as_str() {
            "yaml" | "yml" => Ok(Self::Yaml),
            "json" => Ok(Self::Json),
            _ => Err(FileExtensionError::UnsupportedFileExtension(
                file_name.to_string(),
            )),
        }
    }
}

pub fn is_debug() -> bool {
    cfg!(debug_assertions)
}
