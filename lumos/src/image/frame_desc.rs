//! [`MAX_SIDE`]: the longest side of a frame lumos holds, and the check that holds a size to it.

use common::USize2;
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::image::error::FrameDescError;

/// The longest side a frame may have, 2³⁰ px — some 7000 times a large sensor's.
///
/// Pixel addressing in the resampler and the detector runs in `i32` lanes, and a side this far
/// under `i32::MAX` leaves every coordinate, and a kernel's reach past either edge, inside that
/// range. The decoders refuse a larger image before it is one.
pub const MAX_SIDE: usize = 1 << 30;

/// The descriptor of a frame of `size` and `layout`, or why there is none: no planar image has
/// them, or a side is past [`MAX_SIDE`].
pub const fn checked(size: USize2, layout: ChannelLayout) -> Result<PlanarDesc, FrameDescError> {
    let desc = match PlanarDesc::checked(size, layout) {
        Ok(desc) => desc,
        Err(error) => return Err(FrameDescError::Desc(error)),
    };
    if size.width > MAX_SIDE || size.height > MAX_SIDE {
        return Err(FrameDescError::SidePastMax { size });
    }
    Ok(desc)
}

/// Bytes a frame's flag plane holds: one per pixel, whatever its channel count.
pub(crate) const fn flag_plane_byte_len(desc: PlanarDesc) -> usize {
    desc.pixel_count()
}

/// Bytes a frame of `desc` holds as it decodes: its `f32` planes and its flag plane. The flag
/// plane is charged to every frame, since a decoder that flags saturation, and a calibration that
/// flags its repairs, give one to most frames.
pub(crate) const fn decoded_byte_len(desc: PlanarDesc) -> usize {
    desc.byte_len() + flag_plane_byte_len(desc)
}

/// Asserts that `desc` is a frame's.
///
/// # Panics
/// When a side of `desc` is past [`MAX_SIDE`].
pub(crate) fn assert_frame(desc: PlanarDesc) {
    assert!(
        desc.width() <= MAX_SIDE && desc.height() <= MAX_SIDE,
        "{} has a side past MAX_SIDE",
        desc.size()
    );
}

#[cfg(test)]
mod tests {
    use common::USize2;
    use imaginarium::{ChannelLayout, DescError};

    use crate::image::error::FrameDescError;
    use crate::image::frame_desc::{self, MAX_SIDE};

    /// A planar descriptor's own rule comes first, then the side limit, on either side.
    #[test]
    fn checked_names_the_first_rule_broken() {
        let past = MAX_SIDE + 1;
        for (size, expected) in [
            (
                USize2::new(0, past),
                FrameDescError::Desc(DescError::ZeroWidth),
            ),
            (
                USize2::new(past, 1),
                FrameDescError::SidePastMax {
                    size: USize2::new(past, 1),
                },
            ),
            (
                USize2::new(1, past),
                FrameDescError::SidePastMax {
                    size: USize2::new(1, past),
                },
            ),
        ] {
            assert_eq!(
                frame_desc::checked(size, ChannelLayout::Mono),
                Err(expected),
                "{size}"
            );
        }
        let desc = frame_desc::checked(USize2::new(MAX_SIDE, 1), ChannelLayout::Rgb).unwrap();
        assert_eq!(desc.size(), USize2::new(MAX_SIDE, 1));
    }
}
