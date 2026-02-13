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
    Json,
    Lua,
    Bitcode,
    Toml,
    Lz4,
    ScnText,
}

impl SerdeFormat {
    pub fn all_formats_for_testing() -> [Self; 5] {
        [
            Self::Json,
            Self::Lua,
            Self::Bitcode,
            Self::Lz4,
            Self::ScnText,
        ]
    }

    pub fn from_file_name(file_name: &str) -> FileFormatResult<Self> {
        let ext = get_file_extension(file_name).ok_or(FileExtensionError::MissingFileExtension)?;

        if ext.eq_ignore_ascii_case("json") {
            Ok(Self::Json)
        } else if ext.eq_ignore_ascii_case("lua") {
            Ok(Self::Lua)
        } else if ext.eq_ignore_ascii_case("bin") {
            Ok(Self::Bitcode)
        } else if ext.eq_ignore_ascii_case("lz4") {
            Ok(Self::Lz4)
        } else if ext.eq_ignore_ascii_case("toml") {
            Ok(Self::Toml)
        } else if ext.eq_ignore_ascii_case("scn") {
            Ok(Self::ScnText)
        } else {
            Err(FileExtensionError::UnsupportedFileExtension(
                file_name.to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_file_extension_normal() {
        assert_eq!(get_file_extension("file.json"), Some("json"));
        assert_eq!(get_file_extension("data.lua"), Some("lua"));
        assert_eq!(get_file_extension("path/to/file.toml"), Some("toml"));
    }

    #[test]
    fn test_get_file_extension_none() {
        assert_eq!(get_file_extension("no_extension"), None);
        assert_eq!(get_file_extension(""), None);
    }

    #[test]
    fn test_from_file_name_all_formats() {
        assert_eq!(
            SerdeFormat::from_file_name("a.json").unwrap(),
            SerdeFormat::Json
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.lua").unwrap(),
            SerdeFormat::Lua
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.bin").unwrap(),
            SerdeFormat::Bitcode
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.lz4").unwrap(),
            SerdeFormat::Lz4
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.toml").unwrap(),
            SerdeFormat::Toml
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.scn").unwrap(),
            SerdeFormat::ScnText
        );
    }

    #[test]
    fn test_from_file_name_case_insensitive() {
        assert_eq!(
            SerdeFormat::from_file_name("a.JSON").unwrap(),
            SerdeFormat::Json
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.Lua").unwrap(),
            SerdeFormat::Lua
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.BIN").unwrap(),
            SerdeFormat::Bitcode
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.SCN").unwrap(),
            SerdeFormat::ScnText
        );
    }

    #[test]
    fn test_from_file_name_missing_extension() {
        let err = SerdeFormat::from_file_name("no_ext").unwrap_err();
        assert!(matches!(err, FileExtensionError::MissingFileExtension));
    }

    #[test]
    fn test_from_file_name_unsupported_extension() {
        let err = SerdeFormat::from_file_name("file.xyz").unwrap_err();
        assert!(matches!(
            err,
            FileExtensionError::UnsupportedFileExtension(_)
        ));
    }

    #[test]
    fn test_all_formats_for_testing_count() {
        assert_eq!(SerdeFormat::all_formats_for_testing().len(), 5);
    }

    #[test]
    fn test_from_file_name_with_path() {
        assert_eq!(
            SerdeFormat::from_file_name("/some/path/config.toml").unwrap(),
            SerdeFormat::Toml
        );
    }
}
