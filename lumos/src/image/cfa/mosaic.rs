//! [`Mosaic`]: a color filter pattern over a sensor's photosites.

use imaginarium::ChannelLayout;
use serde::{Deserialize, Serialize};

use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::image::metadata::image_provenance::ColorProvenance;
use common::Vec2us;

/// A pattern of red, green and blue filters over a sensor's photosites, anchored at the origin of
/// the image data it accompanies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mosaic {
    /// 2x2 Bayer pattern
    Bayer(BayerPattern),
    /// 6x6 X-Trans pattern
    XTrans(XTransPattern),
}

impl Mosaic {
    /// The color of the photosite at `pos`.
    #[inline(always)]
    pub const fn color_at(&self, pos: Vec2us) -> CfaColor {
        match self {
            Self::Bayer(pattern) => pattern.color_at(pos),
            Self::XTrans(pattern) => pattern.color_at(pos),
        }
    }

    /// The color slot of the photosite at `pos`: its [`CfaColor::idx`].
    #[inline(always)]
    pub const fn slot_at(&self, pos: Vec2us) -> usize {
        self.color_at(pos).idx()
    }

    /// The pattern's period on each axis: `color_at` repeats every `period` pixels.
    pub(crate) const fn period(&self) -> usize {
        match self {
            Self::Bayer(_) => 2,
            Self::XTrans(_) => 6,
        }
    }

    /// The planes the pattern's colors make: red, green and blue.
    pub const fn color_layout(&self) -> ChannelLayout {
        ChannelLayout::Rgb
    }

    /// Number of distinct colors: 3.
    pub const fn color_count(&self) -> usize {
        self.color_layout().count()
    }

    /// What the color of the planes the pattern's colors make means: the sensor's own RGB.
    pub(crate) const fn planes_color(self) -> ColorProvenance {
        ColorProvenance::SensorRgb
    }
}
