//! [`InputFormat`]: which decoder a file's extension routes it to.

use std::path::Path;

use imaginarium::FileFormat;

use crate::io::image::error::ImageError;
use crate::io::raw::RAW_EXTENSIONS;

/// The extensions of a FITS file.
pub(crate) const FITS_EXTENSIONS: &[&str] = &["fits", "fit"];

/// The container a file's extension names, which decides the decoder a load uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputFormat {
    Fits,
    CameraRaw,
    /// A raster imaginarium reads.
    Raster(FileFormat),
}

impl InputFormat {
    /// The format `path`'s extension names, in any case.
    pub(crate) fn of(path: &Path) -> Result<Self, ImageError> {
        let extension = path.extension().and_then(|extension| extension.to_str());
        let named_by = |list: &[&str]| {
            extension.is_some_and(|extension| {
                list.iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            })
        };
        if named_by(FITS_EXTENSIONS) {
            Ok(Self::Fits)
        } else if named_by(RAW_EXTENSIONS) {
            Ok(Self::CameraRaw)
        } else {
            extension
                .and_then(FileFormat::from_extension)
                .map(Self::Raster)
                .ok_or_else(|| ImageError::UnsupportedFormat {
                    path: path.to_path_buf(),
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use imaginarium::FileFormat;

    use crate::io::image::error::ImageError;
    use crate::io::image::input_format::InputFormat;

    #[test]
    fn the_extension_names_the_decoder_in_any_case() {
        for (path, expected) in [
            ("a.fits", InputFormat::Fits),
            ("a.FIT", InputFormat::Fits),
            ("dir.fits/a.Cr3", InputFormat::CameraRaw),
            ("a.raf", InputFormat::CameraRaw),
            ("a.TIF", InputFormat::Raster(FileFormat::Tiff)),
            ("a.jpeg", InputFormat::Raster(FileFormat::Jpeg)),
        ] {
            assert_eq!(
                InputFormat::of(Path::new(path)).unwrap(),
                expected,
                "{path}"
            );
        }
        for path in ["a.xyz", "no_extension", "fits"] {
            assert!(
                matches!(
                    InputFormat::of(Path::new(path)),
                    Err(ImageError::UnsupportedFormat { path: refused }) if refused == Path::new(path)
                ),
                "{path}"
            );
        }
    }
}
