//! [`CfaType`]: the color filter over a sensor's photosites.

use arrayvec::ArrayVec;
use imaginarium::ChannelLayout;
use serde::{Deserialize, Serialize};

use crate::image::cfa::color_raster::ColorRaster;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::metadata::image_provenance::ColorProvenance;
use common::Size2us;
use glam::USizeVec2;

/// The color filter over a sensor's photosites, anchored at the origin of the image data it
/// accompanies. The one sensor-pattern type: a frame with no filter is [`CfaType::Mono`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfaType {
    /// No CFA pattern (monochrome sensor)
    Mono,
    /// A mosaic of color filters
    Mosaic(Mosaic),
}

impl CfaType {
    /// The longest period a pattern has, X-Trans's: within it every color recurs on every row.
    pub(crate) const MAX_PERIOD: usize = 6;

    /// The color slot of the photosite at `pos`: its
    /// [`CfaColor::idx`](crate::image::cfa::cfa_color::CfaColor::idx) under a mosaic, and 0,
    /// the one slot, on a mono sensor.
    #[inline(always)]
    pub const fn slot_at(&self, pos: USizeVec2) -> usize {
        match self {
            Self::Mono => 0,
            Self::Mosaic(mosaic) => mosaic.slot_at(pos),
        }
    }

    /// The mosaic of colors, `None` on a mono sensor.
    pub const fn mosaic(self) -> Option<Mosaic> {
        match self {
            Self::Mono => None,
            Self::Mosaic(mosaic) => Some(mosaic),
        }
    }

    /// The pattern's period on each axis: `slot_at` repeats every `period` pixels.
    pub(crate) const fn period(&self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Mosaic(mosaic) => mosaic.period(),
        }
    }

    /// The rows of a `size` mosaic a statistic samples so each color keeps at most about
    /// `per_color` samples: whole periods of the pattern's rows, one period in every `step`. A
    /// step over single rows would skip whole rows of the pattern wherever it shares a factor with
    /// the period, and with them every photosite of a color only those rows hold.
    pub(crate) fn sampled_rows(
        &self,
        size: Size2us,
        per_color: usize,
    ) -> impl Iterator<Item = usize> + use<> {
        let period = self.period();
        let step = (size.pixel_count() / (self.color_count() * per_color)).max(1);
        (0..size.height).filter(move |y| (y / period).is_multiple_of(step))
    }

    /// Number of distinct color channels (1 for Mono, 3 for Bayer/X-Trans).
    pub const fn color_count(&self) -> usize {
        self.color_layout().count()
    }

    /// The planes the pattern's colors make: one for a mono sensor, red, green and blue for a
    /// mosaic.
    pub const fn color_layout(&self) -> ChannelLayout {
        match self {
            Self::Mono => ChannelLayout::Mono,
            Self::Mosaic(mosaic) => mosaic.color_layout(),
        }
    }

    /// How many photosites of each color, by [`Self::slot_at`]'s index, a sensor of `size`
    /// holds.
    pub(crate) fn photosites_per_color(&self, size: Size2us) -> ArrayVec<usize, 3> {
        (0..self.color_count())
            .map(|color| ColorRaster::new(self, size, color).count())
            .collect()
    }

    /// What the color of the planes this pattern's colors make means: a demosaic's, or a CFA
    /// drizzle's.
    pub(crate) const fn planes_color(self) -> ColorProvenance {
        match self {
            Self::Mono => ColorProvenance::Monochrome,
            Self::Mosaic(mosaic) => mosaic.planes_color(),
        }
    }
}

impl From<Mosaic> for CfaType {
    fn from(mosaic: Mosaic) -> Self {
        Self::Mosaic(mosaic)
    }
}
