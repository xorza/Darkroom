//! [`StoredLayout`]: what a stored frame's planes hold.

use imaginarium::PlanarDesc;

use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_gain::StoredGain;
use crate::image::flat_gain::GainGrid;

/// Quality planes a warped frame carries beside its image: `coverage` and `confidence`, one
/// image-sized plane each, channel-independent — see `registration::resample::WarpResult` and
/// `frame_store::FrameQuality`.
const FRAME_QUALITY_PLANES: usize = 2;

/// What a stored frame's planes hold: its channels and its quality planes, a flag byte a pixel,
/// and a flat gain grid of `gain_channels` it holds as its own — a source gain the frames share is
/// the run's, priced once. The plan prices a frame by it before the frame exists, and the combine
/// prices the chunks it copies by it once it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredLayout {
    pub(crate) channels: usize,
    pub(crate) quality_planes: usize,
    pub(crate) flags: bool,
    pub(crate) gain_channels: usize,
}

impl StoredLayout {
    /// The layout `frame` has.
    pub(crate) fn of(frame: &StoredFrame) -> Self {
        Self {
            channels: frame.channels.len(),
            quality_planes: frame.quality.count(),
            flags: frame.flags.is_some(),
            gain_channels: frame.flat_gain.as_ref().map_or(0, StoredGain::own_channels),
        }
    }

    /// A frame of `desc` as it decodes: its channels and the flag plane charged to every
    /// frame, since a decoder that flags saturation, and a calibration that flags its repairs, give
    /// one to most. Its gain is a source gain, which the run holds once for every frame.
    pub(crate) const fn decoded(desc: PlanarDesc) -> Self {
        Self {
            channels: desc.channel_count(),
            quality_planes: 0,
            flags: true,
            gain_channels: 0,
        }
    }

    /// A frame of `desc` as the warp stores it: its channels, its coverage and its confidence,
    /// its flags, and a gain grid a channel, charged to every warped frame since lights are divided
    /// by a flat as a rule.
    pub(crate) const fn warped(desc: PlanarDesc) -> Self {
        Self {
            channels: desc.channel_count(),
            quality_planes: FRAME_QUALITY_PLANES,
            flags: true,
            gain_channels: desc.channel_count(),
        }
    }

    /// The bytes a whole frame of `desc` holds in this layout, saturated: a frame of
    /// dimensions a scale made, not a file, can count past `usize`, and is then refused as never
    /// fitting.
    pub(crate) const fn heap_bytes(self, desc: PlanarDesc) -> usize {
        let grid = GainGrid::of(desc.size());
        let pixels = desc.pixel_count();
        self.pixel_bytes()
            .saturating_mul(pixels)
            .saturating_add(grid.bytes(self.gain_channels))
    }

    /// The bytes a spilled combine's chunk copy holds for each pixel of its rows: the channels,
    /// the quality planes and the flags. The flat gain is read where its map lies, so no copy
    /// holds it.
    pub(crate) const fn pixel_bytes(self) -> usize {
        (self.channels + self.quality_planes) * size_of::<f32>() + self.flags as usize
    }
}

#[cfg(test)]
mod tests {
    use imaginarium::{ChannelLayout, PlanarDesc};

    use crate::frame_store::stored_layout::StoredLayout;

    /// What frames hold, counted by hand. A flat gain grid has a node every 4 pixels from the
    /// first, the last at or past the last pixel, 4 bytes each a channel: 9 × 9 takes 3 × 3 nodes,
    /// 3 · 9 · 4 = 108 bytes in color; 13 × 5 takes 4 × 2, 32 bytes in mono.
    ///
    /// A 9 × 9 color frame decodes to 3 planes of 81 pixels and a flag byte each, 1053 B, 13 B a
    /// pixel. Warped it adds coverage and confidence and its gain grid: 5 planes, 1620 B, the 81
    /// flag bytes and 108 of gain, 1809 B; a chunk copies 5 · 4 + 1 = 21 B a pixel, no gain. A
    /// 13 × 5 mono frame warped is 3 planes of 65 pixels, 780 B, 65 flag bytes and 32 of gain,
    /// 877 B, and a chunk copies 3 · 4 + 1 = 13 B a pixel.
    #[test]
    fn a_layout_holds_its_planes_flags_and_gain() {
        let color = PlanarDesc::new((9, 9), ChannelLayout::Rgb);
        let mono = PlanarDesc::new((13, 5), ChannelLayout::Mono);
        for (layout, desc, frame, pixel) in [
            (StoredLayout::decoded(color), color, 1053, 13),
            (StoredLayout::warped(color), color, 1809, 21),
            (StoredLayout::warped(mono), mono, 877, 13),
        ] {
            assert_eq!(layout.heap_bytes(desc), frame, "{layout:?}");
            assert_eq!(layout.pixel_bytes(), pixel, "{layout:?}");
        }
    }
}
