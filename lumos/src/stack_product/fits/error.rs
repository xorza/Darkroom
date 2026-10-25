//! [`ProductError`]: why a stack product file does not read.

use std::path::PathBuf;

use crate::io::error::ImageError;
use crate::io::fits::lumos_extension::LumosExtension;

/// What a FITS file read as a stack product breaks of the product's format.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProductRefusal {
    /// The primary HDU is not a Lumos linear image that states a product version.
    #[error("the primary HDU is not a Lumos stack product")]
    NotAProduct,
    /// A stack product of another version than this reader's, `expected`.
    #[error("unsupported Lumos stack product version {version}; expected {expected}")]
    Version { version: i64, expected: i64 },
    /// An `UNCERT` extension whose `UTYPE` is not the `expected` one.
    #[error(
        "{}: uncertainty type {}; expected {expected}",
        LumosExtension::Uncertainty,
        utype.as_deref().unwrap_or("none")
    )]
    UncertaintyType {
        utype: Option<String>,
        expected: &'static str,
    },
    /// A `MASK` sample, at `idx` of its channel-major space, that is not the image's `NO_DATA`
    /// at its pixel: set where the pixel holds data, clear where it holds none, or other than 0
    /// and 1.
    #[error(
        "{} sample {idx} is not the image's NO_DATA at its pixel",
        LumosExtension::Mask
    )]
    MaskDisagrees { idx: usize },
    /// A stack product that states a uniform coverage and holds a `COVERAGE` extension too.
    #[error(
        "the product states a uniform coverage and holds a {} extension",
        LumosExtension::Coverage
    )]
    CoverageTwice,
}

/// Why [`StackProduct::read_fits`](crate::StackProduct::read_fits) could not read a product.
#[derive(Debug, thiserror::Error)]
pub enum ProductError {
    /// The file did not read as the FITS image a product holds: unreadable, malformed, or one of
    /// its Lumos extensions out of format.
    #[error(transparent)]
    Image(#[from] ImageError),
    /// The file reads as FITS, and breaks the product's format.
    #[error("{}: {refusal}", path.display())]
    Refused {
        path: PathBuf,
        refusal: ProductRefusal,
    },
}
