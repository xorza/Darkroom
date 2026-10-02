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

/// Test-only helpers, gated out of the released surface. Enabled in downstream crates' test
/// targets via `common = { …, features = ["internals"] }`.
#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::file_format::SerdeFormat;

    impl SerdeFormat {
        /// Every format, for a round-trip sweep.
        pub fn all_formats_for_testing() -> [Self; 3] {
            [Self::Ron, Self::Bitcode, Self::Lz4]
        }
    }
}

#[cfg(test)]
mod tests;
