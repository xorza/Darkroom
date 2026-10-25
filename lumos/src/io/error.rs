use common::Size2us;
use imaginarium::RasterFormat;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::image::frame_desc::MAX_SIDE;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::null_policy::NullSamples;
use crate::io::raw::error::RawError;

/// Errors that can occur when loading an astronomical image from disk.
#[derive(Debug, Error)]
pub enum ImageError {
    #[error("image load cancelled for '{path}'")]
    Cancelled { path: PathBuf },

    #[error("failed to load FITS file '{path}': {source}")]
    Fits {
        path: PathBuf,
        source: fits_well::FitsError,
    },

    #[error("unsupported FITS file '{path}': {reason}")]
    FitsUnsupported { path: PathBuf, reason: FitsRefusal },

    #[error("failed to load image '{path}': {source}")]
    Image {
        path: PathBuf,
        source: imaginarium::CodecError,
    },

    #[error("failed to load raw file '{path}': {source}")]
    Raw { path: PathBuf, source: RawError },

    #[error("failed to read file '{path}': {source}")]
    Io { path: PathBuf, source: io::Error },

    #[error("no decoder reads the extension of '{path}'")]
    UnsupportedFormat { path: PathBuf },

    #[error("scientific image input '{path}' was rejected: {reason}")]
    ScientificInputRejected {
        path: PathBuf,
        reason: ScientificRefusal,
    },

    #[error(
        "image '{path}' contains {count} null/non-finite samples, first at linear index \
         {first_idx}, and the load's NullPolicy rejects them"
    )]
    NullSamples {
        path: PathBuf,
        /// Samples, each channel's nulls counted separately.
        count: usize,
        /// In the channel-major sample space `count` belongs to.
        first_idx: usize,
    },
}

impl ImageError {
    /// LibRaw's or the decoder's `source` failure reading `path`: [`Self::Cancelled`] where LibRaw
    /// stopped because the load's cancel token asked it to.
    pub(crate) fn raw(path: &Path, source: RawError) -> Self {
        match source {
            RawError::Cancelled => Self::cancelled(path),
            RawError::Libraw(error) if error.is_cancelled() => Self::cancelled(path),
            source => Self::Raw {
                path: path.to_path_buf(),
                source,
            },
        }
    }

    /// The FITS load of `path` that failed for `error`.
    pub(crate) fn fits_load(path: &Path, error: FitsLoadError) -> Self {
        match error {
            FitsLoadError::Io(source) => Self::Io {
                path: path.to_path_buf(),
                source,
            },
            FitsLoadError::Fits(source) => Self::fits(path, source),
            FitsLoadError::Refused(reason) => Self::fits_unsupported(path, reason),
            FitsLoadError::NullSamples(NullSamples { count, first_idx }) => {
                Self::null_samples(path, count, first_idx)
            }
            FitsLoadError::Cancelled => Self::cancelled(path),
        }
    }

    /// The load of `path` stopped by its cancel token.
    pub(crate) fn cancelled(path: &Path) -> Self {
        Self::Cancelled {
            path: path.to_path_buf(),
        }
    }

    /// fits-well's `source` failure reading `path`.
    pub(crate) fn fits(path: &Path, source: fits_well::FitsError) -> Self {
        Self::Fits {
            path: path.to_path_buf(),
            source,
        }
    }

    /// A FITS file at `path` this decoder refuses, for `reason`.
    fn fits_unsupported(path: &Path, reason: FitsRefusal) -> Self {
        Self::FitsUnsupported {
            path: path.to_path_buf(),
            reason,
        }
    }

    /// The `count` null samples of the image at `path`, the first at `first_idx`, under
    /// [`NullPolicy::Reject`](crate::NullPolicy::Reject).
    pub(super) fn null_samples(path: &Path, count: usize, first_idx: usize) -> Self {
        Self::NullSamples {
            path: path.to_path_buf(),
            count,
            first_idx,
        }
    }

    /// An image at `path` refused as scientific input, for `reason`.
    pub(super) fn scientific_rejection(path: &Path, reason: ScientificRefusal) -> Self {
        Self::ScientificInputRejected {
            path: path.to_path_buf(),
            reason,
        }
    }
}

/// Why lumos could not write an image file.
#[derive(Debug, Error)]
pub enum WriteError {
    /// Creating, writing or publishing the file failed, in lumos or inside fits-well.
    #[error("failed to write the file: {0}")]
    Io(#[from] io::Error),
    /// fits-well refused a keyword, an image or a table it was asked to encode.
    #[error("failed to encode FITS data: {0}")]
    Fits(#[source] fits_well::FitsError),
    /// imaginarium could not encode or write the raster format the path names.
    #[error("failed to write the raster image: {0}")]
    Raster(#[from] imaginarium::CodecError),
    /// No raster format has the extension of the path.
    #[error("no raster format has the extension of '{}'", path.display())]
    RasterExtension { path: PathBuf },
}

/// fits-well's own I/O failure is an I/O failure here too, so a caller finds every one in
/// [`WriteError::Io`].
impl From<fits_well::FitsError> for WriteError {
    fn from(source: fits_well::FitsError) -> Self {
        match source {
            fits_well::FitsError::Io(source) => Self::Io(source),
            source => Self::Fits(source),
        }
    }
}

/// Why an image is refused as scientific input: one case per refusal a caller can tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScientificRefusal {
    /// Camera RAW read as a [`LinearImage`](crate::LinearImage): it is a mosaic until calibrated.
    CameraRaw,
    /// A FITS image whose header declares a mosaic, read as a [`LinearImage`](crate::LinearImage).
    MosaicFits,
    /// A raster read as a [`CfaImage`](crate::CfaImage): no raster decoder states a mosaic.
    RasterCfa,
    /// A raster whose transfer and color transforms are not decoded.
    PreviewOnlyRaster {
        format: RasterFormat,
    },
    /// A raster with a side past [`MAX_SIDE`].
    RasterSideTooLong {
        size: Size2us,
    },
    /// A TIFF of integer samples.
    IntegerRaster,
    AlphaChannel,
}

impl Display for ScientificRefusal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CameraRaw => write!(
                f,
                "camera RAW must be loaded as CfaImage and calibrated before demosaicing"
            ),
            Self::MosaicFits => write!(
                f,
                "mosaic FITS must be loaded as CfaImage and calibrated before demosaicing"
            ),
            Self::RasterCfa => write!(f, "scientific CFA input must be camera RAW or FITS"),
            Self::PreviewOnlyRaster { .. } => write!(
                f,
                "PNG and JPEG are preview-only because their transfer and color transforms are \
                 not decoded"
            ),
            Self::RasterSideTooLong { size } => {
                write!(f, "{size} has a side past {MAX_SIDE} px")
            }
            Self::IntegerRaster => {
                write!(f, "scientific raster input must be a floating-point TIFF")
            }
            Self::AlphaChannel => write!(
                f,
                "scientific raster input must not contain an alpha channel"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use fits_well::error::CompressionParameter;

    use crate::io::error::WriteError;

    /// fits-well's I/O failure lands in [`WriteError::Io`], beside lumos's own; any other
    /// fits-well refusal keeps its type.
    #[test]
    fn a_fits_io_failure_is_an_io_failure() {
        let io_error = fits_well::FitsError::Io(io::Error::other("disk full"));
        assert!(matches!(
            WriteError::from(io_error),
            WriteError::Io(error) if error.to_string() == "disk full"
        ));
        let refusal = fits_well::FitsError::InvalidCompressionParameter {
            parameter: CompressionParameter::TableWithoutCodec,
        };
        let wrapped = WriteError::from(refusal);
        assert!(
            std::error::Error::source(&wrapped).is_some(),
            "the refusal stays the error's cause"
        );
        assert!(matches!(
            wrapped,
            WriteError::Fits(fits_well::FitsError::InvalidCompressionParameter { .. })
        ));
    }
}
