//! Shared decode policy: cancellation, resource ceilings, and format options.

use std::path::Path;

use common::{CancelToken, par};

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::image::preview_image::PreviewImage;
use crate::io::decoder::Peek;
use crate::io::error::{ImageError, ScientificRefusal};
use crate::io::fits::calibration_bundle::error::BundleError;
use crate::io::fits::fits_load_config::FitsLoadConfig;
use crate::io::fits::{calibration_bundle, decode as fits_decode, product as fits_product};
use crate::io::input_format::InputFormat;
use crate::io::null_policy::NullPolicy;
use crate::io::raw::raw_file::RawFile;
use crate::io::{raster, raw};
use crate::stack_product::StackProduct;

/// Cancellation, resource controls, and format policy shared by file decoders.
#[derive(Debug, Clone)]
pub struct LoadCtx {
    /// Cooperative cancellation token polled between bounded decode stages.
    pub cancel: CancelToken,
    /// FITS-specific policy; ignored by non-FITS decoders.
    pub fits: FitsLoadConfig,
    /// What a null sample does to a linear decode: a FITS null, or a floating-point raster's NaN
    /// or infinity.
    pub nulls: NullPolicy,
    /// How many passes an X-Trans demosaic makes; ignored for every other mosaic.
    pub xtrans_passes: MarkesteijnPasses,
    /// How many threads one decode may run inside a decoder that parallelizes itself — LibRaw's
    /// tiled CR3, compressed Fuji and Panasonic v8 decoders. Every thread rayon has for one decode
    /// at a time; a run decoding several frames at once shares them out, so the decodes together
    /// stay within the cores.
    pub decode_threads: usize,
}

impl LoadCtx {
    /// A context polling `cancel`, with strict FITS defaults.
    pub fn new(cancel: CancelToken) -> Self {
        Self {
            cancel,
            fits: FitsLoadConfig::default(),
            nulls: NullPolicy::default(),
            xtrans_passes: MarkesteijnPasses::default(),
            decode_threads: par::threads(),
        }
    }

    /// This context for `slots` decodes running at once: each with an equal share of this one's
    /// threads, one at least — see [`Self::decode_threads`].
    pub(crate) fn for_decode_slots(&self, slots: usize) -> Self {
        Self {
            decode_threads: threads_per_decode(self.decode_threads, slots),
            ..self.clone()
        }
    }

    /// Load an already-linear scientific image: FITS without CFA metadata, or a floating-point
    /// TIFF, taken as linear.
    ///
    /// Camera RAW, mosaic FITS, integer TIFF, alpha TIFF, PNG, and JPEG are refused: load those
    /// with [`Self::cfa`] or [`Self::preview`].
    pub fn linear(&self, path: impl AsRef<Path>) -> Result<LinearImage, ImageError> {
        self.linear_sharing(path.as_ref(), &SharedGains::default())
    }

    /// [`Self::linear`], the flat gain a FITS file stores shared through `gains`.
    pub(crate) fn linear_sharing(
        &self,
        path: &Path,
        gains: &SharedGains,
    ) -> Result<LinearImage, ImageError> {
        self.check_cancelled(path)?;
        match InputFormat::of(path)? {
            InputFormat::Fits => fits_decode::load_linear(path, self, gains),
            InputFormat::CameraRaw => Err(ImageError::scientific_rejection(
                path,
                ScientificRefusal::CameraRaw,
            )),
            InputFormat::Raster(format) => raster::linear(path, format, self),
        }
    }

    /// Load an undemosaiced sensor image from camera RAW or mosaic FITS.
    pub fn cfa(&self, path: impl AsRef<Path>) -> Result<CfaImage, ImageError> {
        self.cfa_sharing(path.as_ref(), &SharedGains::default())
    }

    /// [`Self::cfa`], the flat gain a FITS file stores shared through `gains`.
    pub(crate) fn cfa_sharing(
        &self,
        path: &Path,
        gains: &SharedGains,
    ) -> Result<CfaImage, ImageError> {
        self.check_cancelled(path)?;
        match InputFormat::of(path)? {
            InputFormat::Fits => fits_decode::load_cfa(path, self, gains),
            InputFormat::CameraRaw => raw::load_cfa(path, self),
            InputFormat::Raster(_) => Err(ImageError::scientific_rejection(
                path,
                ScientificRefusal::RasterCfa,
            )),
        }
    }

    /// Load a display or inspection image from FITS, camera RAW, TIFF, PNG, or JPEG.
    pub fn preview(&self, path: impl AsRef<Path>) -> Result<PreviewImage, ImageError> {
        let path = path.as_ref();
        self.check_cancelled(path)?;
        match InputFormat::of(path)? {
            InputFormat::Fits => fits_decode::load_preview(path, self).map(Into::into),
            InputFormat::CameraRaw => raw::load_preview(path, self).map(Into::into),
            InputFormat::Raster(format) => raster::preview(path, format, self),
        }
    }

    /// Load a stack product that [`writer::product_fits`](crate::writer::product_fits) wrote:
    /// its image, every quality plane it holds, and its run report.
    pub fn product(&self, path: impl AsRef<Path>) -> Result<StackProduct, ImageError> {
        let path = path.as_ref();
        self.check_cancelled(path)?;
        fits_product::load(path, self)
    }

    /// Load a calibration bundle that
    /// [`writer::calibration_bundle`](crate::writer::calibration_bundle) wrote, without rebuilding
    /// its prepared flat or defect map: cancellation reads as [`BundleError::Cancelled`]. The file
    /// is streamed, never read into RAM whole.
    ///
    /// # Errors
    ///
    /// [`BundleError`], one variant for each way a file can fail to be a bundle of this version.
    pub fn calibration_masters(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<CalibrationMasters, BundleError> {
        calibration_bundle::load(path.as_ref(), self)
    }

    /// What the CFA frame at `path` is, read from its header without decoding it, with the file
    /// a camera RAW's header read leaves for [`Self::cfa_read`].
    pub(crate) fn cfa_peek(&self, path: &Path) -> Result<Peek<RawFile>, ImageError> {
        self.check_cancelled(path)?;
        match InputFormat::of(path)? {
            InputFormat::Fits => Ok(Peek {
                info: fits_decode::cfa_frame_info(path, self)?,
                read: None,
            }),
            InputFormat::CameraRaw => raw::cfa_peek(path, self),
            InputFormat::Raster(_) => Err(ImageError::scientific_rejection(
                path,
                ScientificRefusal::RasterCfa,
            )),
        }
    }

    /// [`Self::cfa`] of the camera RAW `file`, read from `path` by [`Self::cfa_peek`].
    pub(crate) fn cfa_read(&self, file: RawFile, path: &Path) -> Result<CfaImage, ImageError> {
        self.check_cancelled(path)?;
        raw::load_cfa_read(file, path, self)
    }

    pub(super) fn check_cancelled(&self, path: &Path) -> Result<(), ImageError> {
        if self.cancel.is_cancelled() {
            return Err(ImageError::cancelled(path));
        }
        Ok(())
    }
}

/// The threads each of `slots` decodes running at once gets of `cores`: an equal share, one at
/// least, so the decodes together use the cores and no more.
const fn threads_per_decode(cores: usize, slots: usize) -> usize {
    debug_assert!(slots > 0, "a run decodes in one slot at least");
    let share = cores / slots;
    if share == 0 { 1 } else { share }
}

impl Default for LoadCtx {
    /// A load of one file outside any run, which nothing cancels.
    fn default() -> Self {
        Self::new(CancelToken::never())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sixteen cores over one decode give it all sixteen, over four slots four each, over five
    /// three (15 of 16, never 20), and over more slots than cores one each.
    #[test]
    fn decodes_share_the_cores() {
        for (cores, slots, threads) in [(16, 1, 16), (16, 4, 4), (16, 5, 3), (4, 8, 1), (1, 1, 1)] {
            assert_eq!(
                threads_per_decode(cores, slots),
                threads,
                "{cores} cores over {slots}"
            );
        }
    }
}
