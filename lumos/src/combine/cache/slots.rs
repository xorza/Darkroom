//! [`Slots`]: how the combine indexes a frame's per-channel figures.

use crate::io::image::cfa::CfaType;
use crate::math::vec2us::Vec2us;

/// The slots of a frame's noise and weights: one per channel, or one per colour of a mosaic, whose
/// one channel holds three colours with their own noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Slots {
    mosaic: Option<CfaType>,
    count: usize,
}

impl Slots {
    pub(crate) fn new(cfa_type: Option<CfaType>, channels: usize) -> Self {
        let mosaic = cfa_type.filter(|cfa| matches!(cfa, CfaType::Bayer(_) | CfaType::XTrans(_)));
        Self {
            mosaic,
            count: mosaic.map_or(channels, |cfa| cfa.num_colors()),
        }
    }

    pub(crate) const fn count(self) -> usize {
        self.count
    }

    /// The slot of the pixel at `position` in `channel`.
    pub(crate) const fn slot(&self, channel: usize, position: Vec2us) -> usize {
        match &self.mosaic {
            Some(cfa) => cfa.color_at(position) as usize,
            None => channel,
        }
    }

    /// The channel a slot's pixels are in.
    pub(crate) const fn channel(self, slot: usize) -> usize {
        if self.mosaic.is_some() { 0 } else { slot }
    }
}
