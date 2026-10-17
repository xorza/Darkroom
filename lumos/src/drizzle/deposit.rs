//! [`Deposit`]: which output channels a drizzled frame's samples reach, and under which weights.

use crate::combine::normalization::planned_normalization::SetCoverage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::channel_layout::ChannelLayout;
use crate::image::stackable_image::StackableImage;

/// How a frame's samples reach the output channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deposit {
    /// Each of the frame's channels into the output channel of the same index, all of them under
    /// one weight: a drop lands the same in every channel.
    Channels(ChannelLayout),
    /// A mosaic's one plane, each photosite into the channel of its color alone and under that
    /// channel's own weight, as Siril's CFA drizzle deposits (`cdrizzlebox.c`, `chan =
    /// FC_array(...)`): no color is ever interpolated from another.
    Mosaic(CfaType),
}

impl Deposit {
    /// How `image`'s samples reach the output: by color for a mosaic, by channel otherwise.
    pub(crate) fn of(image: &impl StackableImage) -> Self {
        match image.cfa_type().filter(CfaType::is_mosaic) {
            Some(cfa_type) => Self::Mosaic(cfa_type),
            None => Self::Channels(image.dimensions().layout()),
        }
    }

    /// The output's channels: the frame's own, or the mosaic's colors.
    pub(super) const fn output_layout(self) -> ChannelLayout {
        match self {
            Self::Channels(layout) => layout,
            Self::Mosaic(cfa_type) => cfa_type.color_layout(),
        }
    }

    /// The mosaic whose photosites reach a channel each; `None` for frames whose channels do.
    pub(crate) const fn mosaic(self) -> Option<CfaType> {
        match self {
            Self::Channels(_) => None,
            Self::Mosaic(cfa_type) => Some(cfa_type),
        }
    }

    /// The weight planes the output keeps: one every channel shares, or one per color.
    pub(super) const fn weight_planes(self) -> usize {
        match self {
            Self::Channels(_) => 1,
            Self::Mosaic(cfa_type) => cfa_type.num_colors(),
        }
    }

    /// Where the frames of this deposit cover the output: in every channel at once, or each channel
    /// at the pixels its own color's photosites reached.
    pub(super) const fn set_coverage(self) -> SetCoverage {
        match self {
            Self::Channels(_) => SetCoverage::Common,
            Self::Mosaic(_) => SetCoverage::PerChannel,
        }
    }

    /// The weight plane of output channel `channel`: the shared one, or its color's.
    pub(super) const fn weight_plane(self, channel: usize) -> usize {
        match self {
            Self::Channels(_) => 0,
            Self::Mosaic(_) => channel,
        }
    }
}
