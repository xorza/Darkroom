//! [`Slots`]: how the combine indexes a frame's per-channel figures.

use crate::frame_store::stored_frame::StoredFrame;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_dimensions::ImageDimensions;
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
        let mosaic = cfa_type.filter(CfaType::is_mosaic);
        Self {
            mosaic,
            count: mosaic.map_or(channels, |cfa| cfa.num_colors()),
        }
    }

    /// The slots of `frames`, whose sources the set's facts agreed on, at `dimensions`.
    pub(crate) fn of_frames(frames: &[StoredFrame], dimensions: ImageDimensions) -> Self {
        Self::new(frames[0].source_stats.facts.cfa_type, dimensions.channels())
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

    /// The mosaic whose colours the slots are, `None` when each is a channel.
    pub(crate) const fn mosaic(self) -> Option<CfaType> {
        self.mosaic
    }

    /// The channel a slot's pixels are in.
    pub(crate) const fn channel(self, slot: usize) -> usize {
        if self.mosaic.is_some() { 0 } else { slot }
    }
}
