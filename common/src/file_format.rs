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
pub enum SerdeFormat {
    Yaml,
    Json,
    Lua,
    Bitcode,
    Toml,
    Scn,
    ScnText,
}

impl SerdeFormat {
    pub fn all_formats_for_testing() -> [Self; 6] {
        [
            Self::Yaml,
            Self::Json,
            Self::Lua,
            Self::Bitcode,
            Self::Scn,
            Self::ScnText,
        ]
    }

    pub fn from_file_name(file_name: &str) -> FileFormatResult<Self> {
        let ext = get_file_extension(file_name).ok_or(FileExtensionError::MissingFileExtension)?;

        if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") {
            Ok(Self::Yaml)
        } else if ext.eq_ignore_ascii_case("json") {
            Ok(Self::Json)
        } else if ext.eq_ignore_ascii_case("lua") {
            Ok(Self::Lua)
        } else if ext.eq_ignore_ascii_case("bin") {
            Ok(Self::Bitcode)
        } else if ext.eq_ignore_ascii_case("scn") {
            Ok(Self::Scn)
        } else if ext.eq_ignore_ascii_case("toml") {
            Ok(Self::Toml)
        } else if ext.eq_ignore_ascii_case("scnt") {
            Ok(Self::ScnText)
        } else {
            Err(FileExtensionError::UnsupportedFileExtension(
                file_name.to_string(),
            ))
        }
    }
}
