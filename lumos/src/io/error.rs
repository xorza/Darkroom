use common::USize2;
use imaginarium::RasterFormat;
use std::fmt::{self, Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::error::Cancelled;
use crate::image::frame_desc::MAX_SIDE;
use crate::io::fits::error::FitsLoadError;
use crate::io::null_policy::NullSamples;
use crate::io::raw::error::RawError;

/// A load of the file at `path` that failed, for `kind`.
#[derive(Debug, Error)]
#[error("failed to load '{}'", path.display())]
pub struct ImageError {
    pub path: PathBuf,
    #[source]
    pub kind: LoadError,
}

impl ImageError {
    /// The load of `path` that failed for `kind`.
    pub(crate) fn new(path: &Path, kind: impl Into<LoadError>) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: kind.into(),
        }
    }
}

/// Why a load failed: one case per decoder, each keeping its own error, and the outcomes a caller
/// tells apart whichever decoder met them — a cancel, a refusal as scientific input, nulls the
/// policy refuses — each one case alone, never inside a decoder's.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("cancelled")]
    Cancelled,
    #[error("no decoder reads the file's extension")]
    UnsupportedFormat,
    /// The format cannot serve the entry it was read through.
    #[error("refused as scientific input: {0}")]
    Refused(ScientificRefusal),
    #[error(transparent)]
    NullSamples(NullSamples),
    #[error("failed to read the file")]
    Io(#[source] io::Error),
    #[error("failed to decode the FITS file")]
    Fits(#[source] FitsLoadError),
    #[error("failed to decode the raster")]
    Raster(#[source] RasterLoadError),
    #[error("failed to decode the camera RAW")]
    Raw(#[source] RawError),
}

impl From<io::Error> for LoadError {
    fn from(source: io::Error) -> Self {
        Self::Io(source)
    }
}

impl From<ScientificRefusal> for LoadError {
    fn from(reason: ScientificRefusal) -> Self {
        Self::Refused(reason)
    }
}

impl From<FitsLoadError> for LoadError {
    fn from(source: FitsLoadError) -> Self {
        match source {
            FitsLoadError::Cancelled => Self::Cancelled,
            FitsLoadError::Scientific(reason) => Self::Refused(reason),
            FitsLoadError::NullSamples(nulls) => Self::NullSamples(nulls),
            source => Self::Fits(source),
        }
    }
}

impl From<RasterLoadError> for LoadError {
    fn from(source: RasterLoadError) -> Self {
        match source {
            RasterLoadError::Cancelled => Self::Cancelled,
            RasterLoadError::Refused(reason) => Self::Refused(reason),
            RasterLoadError::NullSamples(nulls) => Self::NullSamples(nulls),
            source => Self::Raster(source),
        }
    }
}

/// LibRaw stopping because the load's cancel token asked it to is the load's cancellation.
impl From<RawError> for LoadError {
    fn from(source: RawError) -> Self {
        match source {
            RawError::Cancelled => Self::Cancelled,
            RawError::Libraw(error) if error.is_cancelled() => Self::Cancelled,
            source => Self::Raw(source),
        }
    }
}

/// Why a raster decode failed.
#[derive(Debug, Error)]
pub enum RasterLoadError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Codec(#[from] imaginarium::CodecError),
    #[error("{0}")]
    Refused(ScientificRefusal),
    #[error("raster is {0}, which has a side past {max} px", max = MAX_SIDE)]
    SideTooLong(USize2),
    #[error(transparent)]
    NullSamples(#[from] NullSamples),
    #[error("cancelled")]
    Cancelled,
}

impl From<Cancelled> for RasterLoadError {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// Why lumos could not write an image file.
#[derive(Debug, Error)]
pub enum WriteError {
    /// Creating, writing or publishing the file failed, in lumos or inside fits-well.
    #[error("failed to write the file")]
    Io(#[from] io::Error),
    /// fits-well refused a keyword, an image or a table it was asked to encode.
    #[error("failed to encode FITS data")]
    Fits(#[source] fits_well::FitsError),
    /// imaginarium could not encode or write the raster format the path names.
    #[error("failed to write the raster image")]
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
