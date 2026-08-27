use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum FileExtensionError {
    #[error("Failed to get file extension")]
    MissingFileExtension,
    #[error("Unsupported file extension for file: {0}")]
    UnsupportedFileExtension(String),
}

pub type FileFormatResult<T> = Result<T, FileExtensionError>;

fn get_file_extension(filename: &str) -> Option<&str> {
    Path::new(filename)
        .extension()
        .and_then(|os_str| os_str.to_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerdeFormat {
    Ron,
    Bitcode,
    Lz4,
}

impl SerdeFormat {
    pub fn from_file_name(file_name: &str) -> FileFormatResult<Self> {
        let ext = get_file_extension(file_name).ok_or(FileExtensionError::MissingFileExtension)?;

        if ext.eq_ignore_ascii_case("ron") {
            Ok(Self::Ron)
        } else if ext.eq_ignore_ascii_case("bin") {
            Ok(Self::Bitcode)
        } else if ext.eq_ignore_ascii_case("lz4") {
            Ok(Self::Lz4)
        } else {
            Err(FileExtensionError::UnsupportedFileExtension(
                file_name.to_string(),
            ))
        }
    }
}

/// Test-only helpers, gated out of the released surface. Enabled in downstream
/// crates' test targets via `common = { …, features = ["internals"] }`.
#[cfg(any(test, feature = "internals"))]
impl SerdeFormat {
    /// Every format, for a round-trip sweep.
    pub fn all_formats_for_testing() -> [Self; 3] {
        [Self::Ron, Self::Bitcode, Self::Lz4]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_file_extension_normal() {
        assert_eq!(get_file_extension("file.ron"), Some("ron"));
        assert_eq!(get_file_extension("path/to/file.bin"), Some("bin"));
    }

    #[test]
    fn test_get_file_extension_none() {
        assert_eq!(get_file_extension("no_extension"), None);
        assert_eq!(get_file_extension(""), None);
    }

    #[test]
    fn test_from_file_name_all_formats() {
        assert_eq!(
            SerdeFormat::from_file_name("a.ron").unwrap(),
            SerdeFormat::Ron
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.bin").unwrap(),
            SerdeFormat::Bitcode
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.lz4").unwrap(),
            SerdeFormat::Lz4
        );
    }

    #[test]
    fn test_from_file_name_case_insensitive() {
        assert_eq!(
            SerdeFormat::from_file_name("a.RON").unwrap(),
            SerdeFormat::Ron
        );
        assert_eq!(
            SerdeFormat::from_file_name("a.BIN").unwrap(),
            SerdeFormat::Bitcode
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
        assert_eq!(SerdeFormat::all_formats_for_testing().len(), 3);
    }

    #[test]
    fn test_rhai_extension_is_unsupported() {
        let err = SerdeFormat::from_file_name("legacy.rhai").unwrap_err();
        assert!(matches!(
            err,
            FileExtensionError::UnsupportedFileExtension(_)
        ));
    }

    #[test]
    fn test_from_file_name_with_path() {
        assert_eq!(
            SerdeFormat::from_file_name("/some/path/config.ron").unwrap(),
            SerdeFormat::Ron
        );
    }
}
