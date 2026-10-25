//! [`FileFormat`]: the image file formats imaginarium reads and writes.

use std::path::Path;

use crate::common::error::{Error, Result};

/// An image file format imaginarium reads and writes, named by the file's extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Png,
    Jpeg,
    Tiff,
}

impl FileFormat {
    pub const ALL: [Self; 3] = [Self::Png, Self::Jpeg, Self::Tiff];

    /// The extensions that name this format, lowercase, the usual one first.
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Png => &["png"],
            Self::Jpeg => &["jpg", "jpeg"],
            Self::Tiff => &["tiff", "tif"],
        }
    }

    /// The format `extension` names, in any case.
    pub fn from_extension(extension: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| {
            format
                .extensions()
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
    }

    /// The format the extension of `path` names.
    pub fn from_path(path: &Path) -> Result<Self> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
            .ok_or_else(|| Error::InvalidExtension(path.to_path_buf()))
    }
}

/// Every extension of every [`FileFormat`], in [`FileFormat::ALL`] order.
pub const SUPPORTED_EXTENSIONS: [&str; extension_count()] = {
    let mut extensions = [""; extension_count()];
    let mut next = 0;
    let mut format = 0;
    while format < FileFormat::ALL.len() {
        let own = FileFormat::ALL[format].extensions();
        let mut index = 0;
        while index < own.len() {
            extensions[next] = own[index];
            next += 1;
            index += 1;
        }
        format += 1;
    }
    extensions
};

const fn extension_count() -> usize {
    let mut count = 0;
    let mut format = 0;
    while format < FileFormat::ALL.len() {
        count += FileFormat::ALL[format].extensions().len();
        format += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::common::error::Error;
    use crate::image::file_format::{FileFormat, SUPPORTED_EXTENSIONS};

    #[test]
    fn extensions_name_their_format_in_any_case() {
        assert_eq!(SUPPORTED_EXTENSIONS, ["png", "jpg", "jpeg", "tiff", "tif"]);
        for format in FileFormat::ALL {
            for extension in format.extensions() {
                assert_eq!(FileFormat::from_extension(extension), Some(format));
                let upper = extension.to_ascii_uppercase();
                assert_eq!(FileFormat::from_extension(&upper), Some(format));
            }
        }
        assert_eq!(FileFormat::from_extension("fits"), None);
        assert_eq!(FileFormat::from_extension(""), None);
        assert_eq!(
            FileFormat::from_path(Path::new("dir.png/frame.Tif")).unwrap(),
            FileFormat::Tiff
        );
        for path in ["frame", "frame.xyz", "png"] {
            assert!(
                matches!(
                    FileFormat::from_path(Path::new(path)),
                    Err(Error::InvalidExtension(refused)) if refused == Path::new(path)
                ),
                "{path}"
            );
        }
    }
}
