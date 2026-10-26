//! [`FitsFile`]: a FITS file open, its image selected and planned, for a load to read.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use common::fs;
use fits_well::FitsReader;
use fits_well::io::StreamReader;

use crate::error::Cancelled;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::io::decoder::Peek;
use crate::io::error::{ImageError, ScientificRefusal};
use crate::io::fits::decode::DecodedFitsImage;
use crate::io::fits::decode::selected_fits::{Checksum, SelectedFits};
use crate::io::fits::error::FitsLoadError;
use crate::io::load_ctx::LoadCtx;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

/// A FITS file open over `R`, and the image of it the load's options select: what a header read
/// leaves for the frame's load, which reads on from the HDU list the reader scanned and the tile
/// index of a compressed image rather than open the file and read them again.
#[derive(Debug)]
pub(crate) struct FitsFile<R = File> {
    reader: StreamReader<R>,
    pub(super) selected: SelectedFits,
}

impl FitsFile {
    /// The FITS file at `path`, open, with the image `ctx`'s options select.
    pub(crate) fn open(path: &Path, ctx: &LoadCtx) -> Result<Self, ImageError> {
        fs::open(path)
            .map_err(FitsLoadError::from)
            .and_then(|file| Self::new(file, ctx))
            .map_err(|error| ImageError::fits_load(path, error))
    }
}

impl<R: Read + Seek> FitsFile<R> {
    /// The FITS file `source` holds, open, with the image `ctx`'s options select.
    pub(crate) fn new(source: R, ctx: &LoadCtx) -> Result<Self, FitsLoadError> {
        Cancelled::check(&ctx.cancel)?;
        let mut reader = FitsReader::open(source)?;
        Cancelled::check(&ctx.cancel)?;
        let selected = SelectedFits::select(&mut reader, ctx)?;
        Ok(Self { reader, selected })
    }

    /// The heap the file holds until its load takes it: what the reader keeps of the selected
    /// image, a compressed image's tile index.
    pub(crate) const fn held_bytes(&self) -> usize {
        self.selected.held_bytes
    }

    /// What the image is as a linear frame, from its header and a compressed image's tile index
    /// alone: the dimensions it decodes to and what its decode holds beside them, by the same
    /// selection, cube reading and limits [`Self::linear`] applies, without reading a sample; with
    /// the file, which [`Self::linear`] reads on from. A header that declares a mosaic is refused,
    /// as the load refuses it.
    pub(crate) fn linear_peek(self) -> Result<Peek<Self>, FitsLoadError> {
        self.refuse_mosaic()?;
        Ok(Peek {
            info: self.selected.linear_frame_info(),
            read: self,
        })
    }

    /// What the image is as the sensor frame [`Self::cfa`] requires, from its header, with the
    /// file, which [`Self::cfa`] reads on from.
    pub(crate) fn cfa_peek(self) -> Result<Peek<Self>, FitsLoadError> {
        Ok(Peek {
            info: self.selected.cfa_frame_info()?,
            read: self,
        })
    }

    /// The image as an already-linear astronomical image, its flat gain shared through `gains`. A
    /// header that declares a mosaic is refused: it loads as a `CfaImage`, to be calibrated before
    /// it is demosaiced.
    pub(crate) fn linear(
        self,
        ctx: &LoadCtx,
        gains: &SharedGains,
    ) -> Result<LinearImage, FitsLoadError> {
        self.refuse_mosaic()?;
        self.read(ctx, Checksum::Judged, gains)?.into_linear()
    }

    /// The image as a sensor frame, its flat gain shared through `gains`.
    pub(crate) fn cfa(self, ctx: &LoadCtx, gains: &SharedGains) -> Result<CfaImage, FitsLoadError> {
        let cfa_type = self.selected.require_cfa()?;
        self.read(ctx, Checksum::Judged, gains)?.into_cfa(cfa_type)
    }

    /// The image for display: a mosaic demosaiced.
    pub(crate) fn preview(self, ctx: &LoadCtx) -> Result<LinearImage, FitsLoadError> {
        let Some(cfa_type) = self.selected.cfa_type else {
            return self
                .read(ctx, Checksum::Judged, &SharedGains::default())?
                .into_linear();
        };
        let cfa = self
            .read(ctx, Checksum::Judged, &SharedGains::default())?
            .into_cfa(cfa_type)?;
        let noise = MosaicNoiseMeasure::of(&cfa).map(MosaicNoiseMeasure::measure);
        Ok(cfa.demosaic(ctx.xtrans_passes, noise, &ctx.cancel)?)
    }

    fn refuse_mosaic(&self) -> Result<(), FitsLoadError> {
        if self.selected.is_mosaic() {
            return Err(FitsLoadError::Scientific(ScientificRefusal::MosaicFits));
        }
        Ok(())
    }

    /// Decode the selected image — see [`SelectedFits::read`].
    fn read(
        self,
        ctx: &LoadCtx,
        checksum: Checksum,
        gains: &SharedGains,
    ) -> Result<DecodedFitsImage, FitsLoadError> {
        let Self {
            mut reader,
            selected,
        } = self;
        selected.read(&mut reader, ctx, checksum, gains)
    }
}
