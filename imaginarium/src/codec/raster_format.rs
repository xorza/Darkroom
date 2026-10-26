//! [`RasterFormat`]: the image file formats imaginarium reads and writes.

use std::fmt;
use std::io::{BufRead, Seek, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::codec;
use crate::error::CodecError;
use crate::interleaved::interleaved_image::InterleavedImage;

/// An image file format imaginarium reads and writes, named by the file's extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RasterFormat {
    Png,
    Jpeg,
    Tiff,
}

impl RasterFormat {
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

    /// The image `input` holds in this format, decoded as it is read: a file whoever owns the
    /// reading opened, or bytes in a [`Cursor`](std::io::Cursor). A header stating more than
    /// `max_bytes` of samples is refused before they are allocated, as the image crate's
    /// `Limits::max_alloc` refuses one: a few bytes of a corrupt or hostile file can state
    /// gigabytes.
    pub fn decode(
        self,
        mut input: impl BufRead + Seek,
        max_bytes: u64,
    ) -> Result<InterleavedImage, CodecError> {
        codec::decode(&mut input, self, max_bytes)
    }

    /// `image` in this format, written into `out`, which the encoder may seek back over.
    pub fn encode(
        self,
        image: &InterleavedImage,
        mut out: impl Write + Seek,
    ) -> Result<(), CodecError> {
        codec::encode(image, self, &mut out)
    }

    /// The format the extension of `path` names, in any case.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
    }
}

/// The format's own name, as `PNG`.
impl fmt::Display for RasterFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
        })
    }
}

/// Every extension of every [`RasterFormat`], in [`RasterFormat::ALL`] order.
pub const SUPPORTED_EXTENSIONS: [&str; extension_count()] = {
    let mut extensions = [""; extension_count()];
    let mut next = 0;
    let mut format = 0;
    while format < RasterFormat::ALL.len() {
        let own = RasterFormat::ALL[format].extensions();
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
    while format < RasterFormat::ALL.len() {
        count += RasterFormat::ALL[format].extensions().len();
        format += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::codec::raster_format::{RasterFormat, SUPPORTED_EXTENSIONS};

    #[test]
    fn extensions_name_their_format_in_any_case() {
        assert_eq!(SUPPORTED_EXTENSIONS, ["png", "jpg", "jpeg", "tiff", "tif"]);
        for format in RasterFormat::ALL {
            for extension in format.extensions() {
                assert_eq!(RasterFormat::from_extension(extension), Some(format));
                let upper = extension.to_ascii_uppercase();
                assert_eq!(RasterFormat::from_extension(&upper), Some(format));
            }
        }
        assert_eq!(RasterFormat::from_extension("fits"), None);
        assert_eq!(RasterFormat::from_extension(""), None);
        assert_eq!(
            RasterFormat::from_path(Path::new("dir.png/frame.Tif")),
            Some(RasterFormat::Tiff)
        );
        for path in ["frame", "frame.xyz", "png"] {
            assert_eq!(RasterFormat::from_path(Path::new(path)), None, "{path}");
        }
        let names = RasterFormat::ALL.map(|format| format.to_string());
        assert_eq!(names, ["PNG", "JPEG", "TIFF"]);
    }
}
