//! [`LumosExtension`]: the image extensions Lumos writes beside an image to describe it.

use std::fmt::{self, Display, Formatter};

use fits_well::image::{Bitpix, Scaling};
use fits_well::io::{Hdu, HduKind};

use crate::image::flat_gain::GainGrid;
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::flags_extension::{FLAGS_EXTNAME, FLAGS_FORMAT, FLAGS_VERSION};
use crate::io::fits::gain_extension::{GAIN_EXTNAME, GAIN_FORMAT, GAIN_VERSION};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use common::USize2;

/// The `LUMOSFMT` and `LUMOSVER` of a stack product's quality planes.
pub(crate) const QUALITY_FORMAT: &str = "QUALPLAN";
pub(crate) const QUALITY_VERSION: i64 = 1;

/// An image extension that describes another image of its file rather than being one; it shows
/// as its `EXTNAME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LumosExtension {
    /// `LUMFLAGS`: the image's [`PixelFlags`](crate::PixelFlags).
    Flags,
    /// `LUMGAIN`: the image's [`FlatGain`](crate::FlatGain).
    Gain,
    /// `MASK`: a stack product's `NO_DATA` pixels, a byte each, as astropy's `CCDData` reads a
    /// mask.
    Mask,
    /// `UNCERT`: a stack product's
    /// [`inverse_variance`](crate::StackProduct::inverse_variance), as `CCDData` reads an
    /// `InverseVariance` uncertainty.
    Uncertainty,
    /// `COVERAGE`: a stack product's [`coverage`](crate::StackProduct::coverage).
    Coverage,
    /// `WEIGHT`: a stack product's [`weight`](crate::StackProduct::weight).
    Weight,
    /// `DISPERSION`: a stack product's [`dispersion`](crate::StackProduct::dispersion).
    Dispersion,
}

impl LumosExtension {
    pub(crate) const ALL: [Self; 7] = [
        Self::Flags,
        Self::Gain,
        Self::Mask,
        Self::Uncertainty,
        Self::Coverage,
        Self::Weight,
        Self::Dispersion,
    ];

    pub(crate) const fn extname(self) -> &'static str {
        match self {
            Self::Flags => FLAGS_EXTNAME,
            Self::Gain => GAIN_EXTNAME,
            Self::Mask => "MASK",
            Self::Uncertainty => "UNCERT",
            Self::Coverage => "COVERAGE",
            Self::Weight => "WEIGHT",
            Self::Dispersion => "DISPERSION",
        }
    }

    /// The `LUMOSFMT` this reader takes.
    pub(crate) const fn format(self) -> &'static str {
        match self {
            Self::Flags => FLAGS_FORMAT,
            Self::Gain => GAIN_FORMAT,
            Self::Mask | Self::Uncertainty | Self::Coverage | Self::Weight | Self::Dispersion => {
                QUALITY_FORMAT
            }
        }
    }

    /// The `LUMOSVER` this reader takes.
    pub(crate) const fn version(self) -> i64 {
        match self {
            Self::Flags => FLAGS_VERSION,
            Self::Gain => GAIN_VERSION,
            Self::Mask | Self::Uncertainty | Self::Coverage | Self::Weight | Self::Dispersion => {
                QUALITY_VERSION
            }
        }
    }

    /// The sample type the extension stores: a byte for flags and a mask, a float for the rest.
    const fn bitpix(self) -> Bitpix {
        match self {
            Self::Flags | Self::Mask => Bitpix::U8,
            Self::Gain | Self::Uncertainty | Self::Coverage | Self::Weight | Self::Dispersion => {
                Bitpix::F32
            }
        }
    }

    /// The scaling the extension is written at beside `image`: the one that puts it in the units
    /// of the image as a reader scales it. A variance is in the image's units squared and an
    /// inverse variance in their inverse square, a weight plane of inverse variances among them;
    /// the others carry none.
    pub(crate) fn scaling(self, image: &DescribedImage) -> Scaling {
        let Some(scale) = image.scale else {
            return Scaling::IDENTITY;
        };
        let bscale = match self {
            Self::Uncertainty => 1.0 / (scale * scale),
            Self::Weight if image.weight_units == WeightUnits::InverseSquare => {
                1.0 / (scale * scale)
            }
            Self::Dispersion => scale * scale,
            _ => return Scaling::IDENTITY,
        };
        Scaling {
            bscale,
            ..Scaling::IDENTITY
        }
    }

    /// Whether `shape` is this extension's over `image`: the flags one plane of its size, the gain
    /// one or three grids over it, the mask the image's own shape, and the other quality planes one
    /// plane of its size or one per channel of an RGB image.
    fn fits(self, shape: &[usize], image: &DescribedImage) -> bool {
        let DescribedImage { size, channels, .. } = *image;
        let plane = |width: usize, height: usize| [width, height] == [size.width, size.height];
        match (self, shape) {
            (Self::Flags, [width, height]) => plane(*width, *height),
            (Self::Gain, [cols, rows, 1 | 3]) => {
                let grid = GainGrid::of(size);
                [*cols, *rows] == [grid.cols, grid.rows]
            }
            (Self::Mask, [width, height]) => plane(*width, *height) && channels == 1,
            (Self::Mask, [width, height, planes]) => {
                plane(*width, *height) && channels > 1 && *planes == channels
            }
            (
                Self::Uncertainty | Self::Coverage | Self::Weight | Self::Dispersion,
                [width, height],
            ) => plane(*width, *height),
            (
                Self::Uncertainty | Self::Coverage | Self::Weight | Self::Dispersion,
                [width, height, 3],
            ) => plane(*width, *height) && channels == 3,
            _ => false,
        }
    }

    /// Refuse an `hdu` of this extension that is not its format's for `image`: another format or
    /// version, not an uncompressed image, another sample type or scaling, or a shape that does
    /// not fit the image.
    pub(crate) fn check(self, hdu: &Hdu, image: &DescribedImage) -> Result<(), FitsLoadError> {
        let header = &hdu.header;
        let format = header.get_text(FORMAT_KEYWORD)?;
        let version = header.get_integer(VERSION_KEYWORD)?;
        if format != Some(self.format()) || version != Some(self.version()) {
            return Err(FitsRefusal::ExtensionFormat {
                extension: self,
                format: format.map(str::to_owned),
                version,
            }
            .into());
        }
        if hdu.kind != HduKind::Image {
            return Err(FitsRefusal::ExtensionNotImage { extension: self }.into());
        }
        let stored = hdu.image_metadata()?;
        if stored.bitpix != self.bitpix() || stored.scaling != self.scaling(image) {
            return Err(FitsRefusal::ExtensionSamples {
                extension: self,
                bitpix: stored.bitpix,
                scaling: stored.scaling,
            }
            .into());
        }
        if !self.fits(stored.shape, image) {
            return Err(FitsRefusal::ExtensionShape {
                extension: self,
                shape: stored.shape.to_vec(),
                image: image.size,
            }
            .into());
        }
        Ok(())
    }

    /// Whether only its `EXTNAME` names the extension: a lumos name no other writer uses. The
    /// quality planes take the names astropy reads, which other pipelines write too, so their
    /// `LUMOSFMT` names them as well.
    const fn named_alone(self) -> bool {
        matches!(self, Self::Flags | Self::Gain)
    }

    /// Whether `header` is this extension's: its `EXTNAME`, and for a plane of a name other
    /// writers use, its `LUMOSFMT`.
    pub(crate) fn names(self, hdu: &Hdu) -> Result<bool, FitsLoadError> {
        Ok(hdu.matches(self.extname())?
            && (self.named_alone() || hdu.header.get_text(FORMAT_KEYWORD)? == Some(self.format())))
    }
}

/// The image an extension describes, as its check reads it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DescribedImage {
    pub(crate) size: USize2,
    pub(crate) channels: usize,
    /// What a sample of the image is worth in the source's units: its domain's scale.
    pub(crate) scale: Option<f64>,
    /// The units of the image's weight plane, which set the scaling it is written at;
    /// [`WeightUnits::Unitless`] for an image that has none.
    pub(crate) weight_units: WeightUnits,
}

/// The units a weight plane is in, as an image's units scale it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WeightUnits {
    /// No units: counts or relative weights, or no weight plane at all.
    Unitless,
    /// The image's units to the power −2: inverse variances.
    InverseSquare,
}

impl Display for LumosExtension {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.extname())
    }
}
