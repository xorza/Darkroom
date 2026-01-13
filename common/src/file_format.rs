use std::path::Path;

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
    Bin,
    Toml,
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
            "bin" => Ok(Self::Bin),
            "toml" => Ok(Self::Toml),
            _ => Err(FileExtensionError::UnsupportedFileExtension(
                file_name.to_string(),
            )),
        }
    }
}
