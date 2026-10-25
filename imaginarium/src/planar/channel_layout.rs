//! [`ChannelLayout`]: the planes a planar image holds.

use crate::color::color_format::ChannelCount;
use crate::error::DescError;

/// The planes a planar image holds: one of a single channel, or three of red, green and blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelLayout {
    Mono,
    Rgb,
}

impl ChannelLayout {
    /// How many planes the layout holds.
    pub const fn count(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Rgb => 3,
        }
    }
}

/// The layout of `count` planes: 1 is mono, 3 is RGB.
impl TryFrom<usize> for ChannelLayout {
    type Error = DescError;

    fn try_from(count: usize) -> Result<Self, DescError> {
        match count {
            1 => Ok(Self::Mono),
            3 => Ok(Self::Rgb),
            _ => Err(DescError::Channels { count }),
        }
    }
}

/// The interleaved channels of the layout's planes.
impl From<ChannelLayout> for ChannelCount {
    fn from(layout: ChannelLayout) -> Self {
        match layout {
            ChannelLayout::Mono => Self::Mono,
            ChannelLayout::Rgb => Self::Rgb,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::color::color_format::ChannelCount;
    use crate::error::DescError;
    use crate::planar::channel_layout::ChannelLayout;

    #[test]
    fn a_layout_is_one_plane_or_three() {
        for count in [0, 2, 4] {
            assert_eq!(
                ChannelLayout::try_from(count),
                Err(DescError::Channels { count })
            );
        }
        for (layout, count, channels) in [
            (ChannelLayout::Mono, 1, ChannelCount::Mono),
            (ChannelLayout::Rgb, 3, ChannelCount::Rgb),
        ] {
            assert_eq!(layout.count(), count);
            assert_eq!(ChannelLayout::try_from(count), Ok(layout));
            assert_eq!(ChannelCount::from(layout), channels);
        }
    }
}
