//! [`Slots`]: how the combine indexes a frame's per-channel figures.

use crate::frame_store::stored_frame::StoredFrame;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::image_dimensions::ImageDimensions;
use crate::math::vec2us::Vec2us;

/// The slots of a frame's noise and weights: one per channel, or one per color of a mosaic, whose
/// one channel holds three colors with their own noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Slots {
    mosaic: Option<Mosaic>,
    count: usize,
}

impl Slots {
    pub(crate) fn new(mosaic: Option<Mosaic>, channels: usize) -> Self {
        Self {
            mosaic,
            count: mosaic.map_or(channels, |mosaic| mosaic.color_count()),
        }
    }

    /// The slots of `frames`, whose sources the set's facts agreed on, at `dimensions`.
    pub(crate) fn of_frames(frames: &[StoredFrame], dimensions: ImageDimensions) -> Self {
        Self::new(
            frames[0].facts.cfa_type.and_then(CfaType::mosaic),
            dimensions.channels(),
        )
    }

    pub(crate) const fn count(self) -> usize {
        self.count
    }

    /// The slot of the pixel at `pos` in `channel`.
    pub(crate) const fn slot(&self, channel: usize, pos: Vec2us) -> usize {
        match &self.mosaic {
            Some(mosaic) => mosaic.slot_at(pos),
            None => channel,
        }
    }

    /// The mosaic whose colors the slots are, `None` when each is a channel.
    pub(crate) const fn mosaic(self) -> Option<Mosaic> {
        self.mosaic
    }

    /// The channel a slot's pixels are in.
    pub(crate) const fn channel(self, slot: usize) -> usize {
        if self.mosaic.is_some() { 0 } else { slot }
    }
}
