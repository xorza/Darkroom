//! [`CfaType`]: the color filter over a sensor's photosites.

use arrayvec::ArrayVec;
use serde::{Deserialize, Serialize};

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::color_raster::ColorRaster;
use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_provenance::ColorProvenance;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The color filter over a sensor's photosites, anchored at the origin of the image data it
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
    /// The longest period a pattern has, X-Trans's: within it every color recurs on every row.
    pub(crate) const MAX_PERIOD: usize = 6;

    /// The color slot of the photosite at `pos`: its
    /// [`CfaColor::index`](crate::image::cfa::cfa_color::CfaColor::index) under a mosaic, and 0,
    /// the one slot, on a mono sensor.
    #[inline(always)]
    pub const fn slot_at(&self, pos: Vec2us) -> usize {
        match self {
            Self::Mono => 0,
            Self::Bayer(pattern) => pattern.color_at(pos).index(),
            Self::XTrans(pattern) => pattern.color_at(pos).index(),
        }
    }

    /// Whether the pattern is a mosaic of colors: Bayer or X-Trans, not mono.
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
        let step = (size.pixel_count() / (self.num_colors() * per_color)).max(1);
        (0..size.height).filter(move |y| (y / period).is_multiple_of(step))
    }

    /// Number of distinct color channels (1 for Mono, 3 for Bayer/X-Trans).
    pub const fn num_colors(&self) -> usize {
        self.color_layout().count()
    }

    /// The planes the pattern's colors make: one for a mono sensor, red, green and blue for a
    /// mosaic.
    pub const fn color_layout(&self) -> ChannelLayout {
        match self {
            Self::Mono => ChannelLayout::Mono,
            Self::Bayer(_) | Self::XTrans(_) => ChannelLayout::Rgb,
        }
    }

    /// How many photosites of each color, by [`Self::slot_at`]'s index, a sensor of `size`
    /// holds.
    pub(crate) fn photosites_per_color(&self, size: Size2us) -> ArrayVec<usize, 3> {
        (0..self.num_colors())
            .map(|color| ColorRaster::new(self, size, color).count())
            .collect()
    }

    /// What the color of the planes this pattern's colors make means: a demosaic's, or a CFA
    /// drizzle's.
    pub(crate) const fn planes_color(self) -> ColorProvenance {
        match self {
            Self::Mono => ColorProvenance::Monochrome,
            Self::Bayer(_) | Self::XTrans(_) => ColorProvenance::SensorRgb,
        }
    }
}
