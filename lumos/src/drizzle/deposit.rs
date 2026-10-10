//! [`Deposit`]: which output channels a drizzled frame's samples reach, and under which weights.

use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaType;

/// How a frame's samples reach the output channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deposit {
    /// Each of the frame's channels into the output channel of the same index, all of them under
    /// one weight: a drop lands the same in every channel.
    Channels(usize),
    /// A mosaic's one plane, each photosite into the channel of its colour alone and under that
    /// channel's own weight, as Siril's CFA drizzle deposits (`cdrizzlebox.c`, `chan =
    /// FC_array(...)`): no colour is ever interpolated from another.
    Mosaic(CfaType),
}

impl Deposit {
    /// How `image`'s samples reach the output: by colour for a mosaic, by channel otherwise.
    pub(crate) fn of(image: &impl StackableImage) -> Self {
        match image.cfa_type().filter(CfaType::is_mosaic) {
            Some(cfa_type) => Self::Mosaic(cfa_type),
            None => Self::Channels(image.dimensions().channels()),
        }
    }

    /// The output's channels: the frame's own, or the mosaic's colours.
    pub(crate) const fn output_channels(self) -> usize {
        match self {
            Self::Channels(channels) => channels,
            Self::Mosaic(cfa_type) => cfa_type.num_colors(),
        }
    }

    /// The mosaic whose photosites reach a channel each; `None` for frames whose channels do.
    pub(crate) const fn mosaic(self) -> Option<CfaType> {
        match self {
            Self::Channels(_) => None,
            Self::Mosaic(cfa_type) => Some(cfa_type),
        }
    }

    /// The weight planes the output keeps: one every channel shares, or one per colour.
    pub(crate) const fn weight_planes(self) -> usize {
        match self {
            Self::Channels(_) => 1,
            Self::Mosaic(cfa_type) => cfa_type.num_colors(),
        }
    }

    /// The weight plane of output channel `channel`: the shared one, or its colour's.
    pub(crate) const fn weight_plane(self, channel: usize) -> usize {
        match self {
            Self::Channels(_) => 0,
            Self::Mosaic(_) => channel,
        }
    }
}
