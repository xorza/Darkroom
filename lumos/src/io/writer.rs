//! Writing the image types to files: what [`LoadCtx`](crate::LoadCtx) reads back.

use std::path::Path;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::linear_image::LinearImage;
use crate::io::error::WriteError;
use crate::io::fits::calibration_bundle::error::BundleError;
use crate::io::fits::{
    calibration_bundle, cfa as fits_cfa, linear as fits_linear, product as fits_product,
};
use crate::io::raster;
use crate::stack_product::StackProduct;

/// Write `image` to `path` as the raster format its extension names: PNG, JPEG or TIFF.
pub fn raster(image: &LinearImage, path: impl AsRef<Path>) -> Result<(), WriteError> {
    raster::save(path.as_ref(), image)
}

/// Write `image`, a sensor-domain image, to `path` as a checksummed floating-point FITS file, with
/// its flags in a `LUMFLAGS` extension when they hold more than the NaN of a sample with no data.
pub fn cfa_fits(image: &CfaImage, path: impl AsRef<Path>) -> Result<(), WriteError> {
    fits_cfa::save(path.as_ref(), image)
}

/// Write `masters` to `path` as a versioned, checksummed multi-extension FITS bundle: the flat
/// prepared and the dark with its bias state, so
/// [`LoadCtx::calibration_masters`](crate::LoadCtx::calibration_masters) repeats neither
/// the flat's preparation nor the defect detection.
pub fn calibration_bundle(
    masters: &CalibrationMasters,
    path: impl AsRef<Path>,
) -> Result<(), BundleError> {
    calibration_bundle::save(path.as_ref(), masters)
}

/// Write `image`, a linear image, to `path` as a checksummed floating-point FITS file: a plane or
/// an RGB cube, with its flags and flat gain in `LUMFLAGS` and `LUMGAIN` extensions beside it.
pub fn linear_fits(image: &LinearImage, path: impl AsRef<Path>) -> Result<(), WriteError> {
    fits_linear::save(path.as_ref(), image)
}

/// Write `product` to `path` as one checksummed FITS file that astropy's `CCDData.read` loads:
/// the image as by [`linear_fits`], its `NO_DATA` pixels as `MASK`, its inverse variance as
/// `UNCERT`, and its coverage, weight and dispersion as `COVERAGE`, `WEIGHT` and `DISPERSION`;
/// [`LoadCtx::product`](crate::LoadCtx::product) reads it back.
pub fn product_fits(product: &StackProduct, path: impl AsRef<Path>) -> Result<(), WriteError> {
    fits_product::save(path.as_ref(), product)
}
