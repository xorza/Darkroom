//! [`CfaType`]: the colour filter over a sensor's photosites.

use arrayvec::ArrayVec;
use serde::{Deserialize, Serialize};

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::demosaic::bayer::rcd;
use crate::image::cfa::demosaic::xtrans::markesteijn;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::image::error::XTransPatternError;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_provenance::{ColorProvenance, DemosaicProvenance};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::memory::footprint::Footprint;

/// The colour filter over a sensor's photosites, anchored at the origin of the image data it
/// accompanies. The one sensor-pattern type: a frame with no filter is [`CfaType::Mono`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfaType {
    /// No CFA pattern (monochrome sensor)
    Mono,
    /// 2x2 Bayer pattern
    Bayer(CfaPattern),
    /// 6x6 X-Trans pattern
    XTrans(XTransPattern),
}

impl CfaType {
    /// The colour slot of the photosite at `pos`: its
    /// [`CfaColour::index`](crate::image::cfa::cfa_colour::CfaColour::index) under a mosaic, and 0,
    /// the one slot, on a mono sensor.
    #[inline(always)]
    pub const fn color_at(&self, pos: Vec2us) -> usize {
        match self {
            Self::Mono => 0,
            Self::Bayer(pattern) => pattern.color_at(pos).index(),
            Self::XTrans(pattern) => pattern.color_at(pos).index(),
        }
    }

    /// Whether the pattern is a mosaic of colours: Bayer or X-Trans, not mono.
    pub(crate) const fn is_mosaic(&self) -> bool {
        matches!(self, Self::Bayer(_) | Self::XTrans(_))
    }

    /// The pattern's period on each axis: `color_at` repeats every `period` pixels.
    pub(crate) const fn period(&self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Bayer(_) => 2,
            Self::XTrans(_) => 6,
        }
    }

    /// The rows of a `size` mosaic a statistic samples so each colour keeps at most about
    /// `per_colour` samples: whole periods of the pattern's rows, one period in every `step`. A
    /// step over single rows would skip whole rows of the pattern wherever it shares a factor with
    /// the period, and with them every photosite of a colour only those rows hold.
    pub(crate) fn sampled_rows(
        &self,
        size: Size2us,
        per_colour: usize,
    ) -> impl Iterator<Item = usize> + use<> {
        let period = self.period();
        let step = (size.pixel_count() / (self.num_colors() * per_colour)).max(1);
        (0..size.height).filter(move |y| (y / period).is_multiple_of(step))
    }

    /// Number of distinct color channels (1 for Mono, 3 for Bayer/X-Trans).
    pub const fn num_colors(&self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Bayer(_) | Self::XTrans(_) => 3,
        }
    }

    /// How many photosites of each colour, by [`Self::color_at`]'s index, a sensor of `size`
    /// holds: each phase of the pattern counted over the columns and rows it falls on.
    pub(crate) fn photosites_per_colour(&self, size: Size2us) -> ArrayVec<usize, 3> {
        let period = self.period();
        let mut counts: ArrayVec<usize, 3> = (0..self.num_colors()).map(|_| 0).collect();
        for y in 0..period {
            for x in 0..period {
                let columns = size.width.saturating_sub(x).div_ceil(period);
                let rows = size.height.saturating_sub(y).div_ceil(period);
                counts[self.color_at(Vec2us::new(x, y))] += columns * rows;
            }
        }
        counts
    }

    /// The mosaic a camera-RAW sensor delivers, from LibRaw's `filters`, `colors` and `cdesc` and
    /// its visible-origin X-Trans pattern, or `None` when LibRaw has to produce the image itself.
    ///
    /// `None` covers `filters == 0` with three colours — a linear DNG or sRAW, whose samples LibRaw
    /// unpacks already per pixel and so never as a mosaic — a filter word that is neither X-Trans
    /// nor a 2×2 Bayer phase, and any sensor whose colours are not LibRaw's red, green and blue: a
    /// word reads colour 3 as a second green only where `cdesc` says so, and Sony's RGBE or Nikon's
    /// CMYG laid out in a Bayer word's phases would otherwise demosaic as RGB with the wrong
    /// filters. An X-Trans sensor whose layout is not one is an error: the file is corrupt, not
    /// exotic.
    pub(crate) fn from_libraw(
        filters: u32,
        colors: i32,
        cdesc: [u8; 4],
        xtrans: [[u8; 6]; 6],
    ) -> Result<Option<Self>, XTransPatternError> {
        if colors == 1 {
            return Ok(Some(Self::Mono));
        }
        if colors != 3 || &cdesc != b"RGBG" {
            return Ok(None);
        }
        Ok(match filters {
            0 => None,
            // LibRaw's marker for the 6×6 X-Trans layout, whose pattern it keeps apart.
            libraw_sys::LIBRAW_XTRANS => Some(Self::XTrans(XTransPattern::new(xtrans)?)),
            _ => CfaPattern::from_filters(filters).map(Self::Bayer),
        })
    }

    /// What a demosaic of a `dimensions` frame of this pattern holds on `workers`.
    pub(crate) const fn demosaic_footprint(
        self,
        dimensions: ImageDimensions,
        workers: usize,
    ) -> Footprint {
        match self {
            Self::Mono => {
                Footprint::kept(dimensions.pixel_count().saturating_mul(size_of::<f32>()))
            }
            Self::Bayer(_) => rcd::footprint(dimensions.size(), workers),
            Self::XTrans(_) => markesteijn::footprint(dimensions.size(), workers),
        }
    }

    /// The demosaic lumos runs for this pattern, an X-Trans one with `passes`.
    pub(crate) const fn demosaic_provenance(self, passes: MarkesteijnPasses) -> DemosaicProvenance {
        match self {
            Self::Mono => DemosaicProvenance::None,
            Self::Bayer(_) => DemosaicProvenance::LumosRcd,
            Self::XTrans(_) => DemosaicProvenance::LumosMarkesteijn { passes },
        }
    }

    /// How far the demosaic of this pattern, an X-Trans one with `passes`, reads from an output
    /// pixel: an input photosite this far away can change it. Both demosaics decide directions
    /// from neighbourhoods of interpolated values, so the reach is the chained steps', not one
    /// kernel's, and each further Markesteijn pass interpolates again from interpolated colours.
    /// An impulse on random texture moves pixels up to 10 away by more than 2⁻¹⁶ of itself for
    /// RCD, and changes pixels up to 11 and 16 away for one and three Markesteijn passes
    /// (`the_demosaic_support_bounds_every_impulse_response`).
    pub(crate) const fn demosaic_support(self, passes: MarkesteijnPasses) -> usize {
        match (self, passes) {
            (Self::Mono, _) => 0,
            (Self::Bayer(_), _) => 10,
            (Self::XTrans(_), MarkesteijnPasses::One) => 11,
            (Self::XTrans(_), MarkesteijnPasses::Three) => 16,
        }
    }

    /// What the colour of a demosaiced frame of this pattern means.
    pub(crate) const fn demosaiced_color(self) -> ColorProvenance {
        match self {
            Self::Mono => ColorProvenance::Monochrome,
            Self::Bayer(_) | Self::XTrans(_) => ColorProvenance::SensorRgb,
        }
    }
}
