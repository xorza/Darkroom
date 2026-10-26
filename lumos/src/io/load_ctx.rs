//! Shared decode policy: cancellation, resource ceilings, and format options.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use common::{CancelToken, fs, par};

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::io::decoder::Peek;
use crate::io::decoder::cfa_decoder::CfaRead;
use crate::io::error::{ImageError, RasterLoadError, ScientificRefusal};
use crate::io::fits::decode::fits_file::FitsFile;
use crate::io::fits::error::FitsLoadError;
use crate::io::fits::fits_load_config::FitsLoadConfig;
use crate::io::input_format::InputFormat;
use crate::io::null_policy::NullPolicy;
use crate::io::{raster, raw};

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
            InputFormat::Fits => self.fits(path, |file| file.linear(self, gains)),
            InputFormat::CameraRaw => Err(ImageError::scientific_rejection(
                path,
                ScientificRefusal::CameraRaw,
            )),
            InputFormat::Raster(format) => {
                self.raster(path, |source| raster::linear(source, format, self))
            }
        }
    }

    /// [`Self::linear_sharing`] of the FITS `file` a header read opened from `path`.
    pub(crate) fn linear_read(
        &self,
        file: FitsFile,
        path: &Path,
        gains: &SharedGains,
    ) -> Result<LinearImage, ImageError> {
        self.check_cancelled(path)?;
        file.linear(self, gains)
            .map_err(|error| ImageError::fits_load(path, error))
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
            InputFormat::Fits => self.fits(path, |file| file.cfa(self, gains)),
            InputFormat::CameraRaw => raw::load_cfa(path, self),
            InputFormat::Raster(_) => Err(ImageError::scientific_rejection(
                path,
                ScientificRefusal::RasterCfa,
            )),
        }
    }

    /// Load a display or inspection image from FITS, camera RAW, TIFF, PNG, or JPEG, as planes: a
    /// mosaic demosaiced, a raster in `f32` with its alpha dropped.
    pub fn preview(&self, path: impl AsRef<Path>) -> Result<LinearImage, ImageError> {
        let path = path.as_ref();
        self.check_cancelled(path)?;
        match InputFormat::of(path)? {
            InputFormat::Fits => self.fits(path, |file| file.preview(self)),
            InputFormat::CameraRaw => raw::load_preview(path, self),
            InputFormat::Raster(format) => {
                self.raster(path, |source| raster::preview(source, format, self))
            }
        }
    }

    /// What the CFA frame at `path` is, read from its header without decoding it, with what the
    /// header read leaves for [`Self::cfa_read`].
    pub(crate) fn cfa_peek(&self, path: &Path) -> Result<Peek<CfaRead>, ImageError> {
        self.check_cancelled(path)?;
        match InputFormat::of(path)? {
            InputFormat::Fits => Ok(self
                .fits(path, FitsFile::cfa_peek)?
                .map(|file| CfaRead::Fits(Box::new(file)))),
            InputFormat::CameraRaw => Ok(raw::cfa_peek(path, self)?.map(CfaRead::Raw)),
            InputFormat::Raster(_) => Err(ImageError::scientific_rejection(
                path,
                ScientificRefusal::RasterCfa,
            )),
        }
    }

    /// [`Self::cfa_sharing`] of what [`Self::cfa_peek`] read of `path`.
    pub(crate) fn cfa_read(
        &self,
        read: CfaRead,
        path: &Path,
        gains: &SharedGains,
    ) -> Result<CfaImage, ImageError> {
        self.check_cancelled(path)?;
        match read {
            CfaRead::Raw(file) => raw::load_cfa_read(file, path, self),
            CfaRead::Fits(file) => file
                .cfa(self, gains)
                .map_err(|error| ImageError::fits_load(path, error)),
        }
    }

    /// `load` of the FITS file at `path`, open with the image this context selects.
    pub(crate) fn fits<T>(
        &self,
        path: &Path,
        load: impl FnOnce(FitsFile) -> Result<T, FitsLoadError>,
    ) -> Result<T, ImageError> {
        load(FitsFile::open(path, self)?).map_err(|error| ImageError::fits_load(path, error))
    }

    /// `load` of the raster at `path`, read through a buffer.
    fn raster<T>(
        &self,
        path: &Path,
        load: impl FnOnce(BufReader<File>) -> Result<T, RasterLoadError>,
    ) -> Result<T, ImageError> {
        fs::open(path)
            .map_err(RasterLoadError::from)
            .and_then(|file| load(BufReader::new(file)))
            .map_err(|error| ImageError::raster_load(path, error))
    }

    pub(crate) fn check_cancelled(&self, path: &Path) -> Result<(), ImageError> {
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
