//! [`StoredLayout`]: what a stored frame's planes hold.

use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_gain::StoredGain;
use crate::image::flat_gain;
use crate::image::flat_gain::GainGrid;
use crate::image::image_dimensions::ImageDimensions;

/// Quality planes a warped frame carries beside its image: `coverage` and `confidence`, one
/// image-sized plane each, channel-independent — see `registration::resample::WarpResult` and
/// `frame_store::FrameQuality`.
const FRAME_QUALITY_PLANES: usize = 2;

/// What a stored frame's planes hold: its channels and its quality planes, a flag byte a pixel,
/// and a flat gain grid of `gain_channels`. The plan prices a frame by it before the frame exists,
/// and the combine prices the chunks it reads by it once it does.
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
            gain_channels: frame.flat_gain.as_ref().map_or(0, StoredGain::channels),
        }
    }

    /// A frame of `dimensions` as it decodes: its channels and the flag plane charged to every
    /// frame, since a decoder that flags saturation, and a calibration that flags its repairs, give
    /// one to most.
    pub(crate) const fn decoded(dimensions: ImageDimensions) -> Self {
        Self {
            channels: dimensions.channels(),
            quality_planes: 0,
            flags: true,
            gain_channels: 0,
        }
    }

    /// A frame of `dimensions` as the warp stores it: its channels, its coverage and its confidence,
    /// its flags, and a gain grid a channel, charged to every warped frame since lights are divided
    /// by a flat as a rule.
    pub(crate) const fn warped(dimensions: ImageDimensions) -> Self {
        Self {
            channels: dimensions.channels(),
            quality_planes: FRAME_QUALITY_PLANES,
            flags: true,
            gain_channels: dimensions.channels(),
        }
    }

    /// The bytes a whole frame of `dimensions` holds in this layout.
    pub(crate) const fn heap_bytes(self, dimensions: ImageDimensions) -> usize {
        let grid = GainGrid::of(dimensions.size());
        let pixels = dimensions.pixel_count();
        (self.channels + self.quality_planes) * size_of::<f32>() * pixels
            + self.flags as usize * pixels
            + self.gain_channels * grid.columns * grid.rows * size_of::<f32>()
    }

    /// The bytes a chunk of this layout reads for each pixel of its rows.
    ///
    /// A flat gain grid holds a node per `STEP²` pixels of each channel, 3/4 of a byte a pixel at
    /// most. Rounded up to a byte it also covers the two node rows a chunk of the fewest rows reads
    /// past its own: 64 rows read at most 18 node rows of `W/4 + 1.75` nodes, `54W + 378` bytes in
    /// color, under `64W` from 38 columns. A frame under 64 rows, read whole, is too small to
    /// matter.
    pub(crate) const fn pixel_bytes(self) -> usize {
        (self.channels + self.quality_planes) * size_of::<f32>()
            + self.flags as usize
            + (self.gain_channels * size_of::<f32>()).div_ceil(flat_gain::STEP * flat_gain::STEP)
    }
}

#[cfg(test)]
mod tests {
    use crate::frame_store::stored_layout::StoredLayout;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;

    /// What frames hold, counted by hand. A flat gain grid has a node every 4 pixels from the
    /// first, the last at or past the last pixel, 4 bytes each a channel: 9 × 9 takes 3 × 3 nodes,
    /// 3 · 9 · 4 = 108 bytes in color; 13 × 5 takes 4 × 2, 32 bytes in mono.
    ///
    /// A 9 × 9 color frame decodes to 3 planes of 81 pixels and a flag byte each, 1053 B, 13 B a
    /// pixel. Warped it adds coverage and confidence and its gain grid: 5 planes, 1620 B, the 81
    /// flag bytes and 108 of gain, 1809 B; a row reads 5 · 4 + 1 B a pixel and the gain's 12 B
    /// over 16 pixels, rounded up to 1: 22. A 13 × 5 mono frame warped is 3 planes of 65 pixels,
    /// 780 B, 65 flag bytes and 32 of gain, 877 B, and 12 + 1 + 1 = 14 B a pixel.
    #[test]
    fn a_layout_holds_its_planes_flags_and_gain() {
        let color = ImageDimensions::new((9, 9), ChannelLayout::Rgb);
        let mono = ImageDimensions::new((13, 5), ChannelLayout::Mono);
        for (layout, dimensions, frame, pixel) in [
            (StoredLayout::decoded(color), color, 1053, 13),
            (StoredLayout::warped(color), color, 1809, 22),
            (StoredLayout::warped(mono), mono, 877, 14),
        ] {
            assert_eq!(layout.heap_bytes(dimensions), frame, "{layout:?}");
            assert_eq!(layout.pixel_bytes(), pixel, "{layout:?}");
        }
    }
}
