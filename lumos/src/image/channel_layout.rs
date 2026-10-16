//! [`ChannelLayout`]: the channels an image holds.

use crate::image::error::DimensionsError;

/// The channels an image holds: one plane of luminance, or three of red, green and blue.
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
    type Error = DimensionsError;

    fn try_from(count: usize) -> Result<Self, DimensionsError> {
        match count {
            1 => Ok(Self::Mono),
            3 => Ok(Self::Rgb),
            _ => Err(DimensionsError::Channels { count }),
        }
    }
}
