//! [`DisplayStretch`]: how a picture of an image maps its samples to the screen.

use std::str::FromStr;

use lumos::{ColorMode, FrameDescError, OpError, Stretch, StretchMethod};
use scenarium::{DataType, EnumId, EnumVariants};

/// The id of [`DisplayStretch`]'s enum type, which saved documents name: fixed for the life of
/// the type.
pub const DISPLAY_STRETCH_TYPE_ID: EnumId = EnumId::literal("a2400ac8-e657-4a83-8382-ea9b36c15b0c");

/// The data type of a port that takes a [`DisplayStretch`].
pub const DISPLAY_STRETCH_DATA_TYPE: DataType = DataType::Enum(DISPLAY_STRETCH_TYPE_ID);

/// How a picture of an image maps its samples to the screen; it never changes them. A linear
/// 32-bit frame shows nearly black as it is, so a look at one takes a screen stretch, as
/// PixInsight's screen transfer function transforms the screen rendition of an image and leaves
/// its pixel data as it is
/// ([STF](https://pixinsight.com/doc/tools/ScreenTransferFunction/ScreenTransferFunction.html)).
/// A preview node's own input, as a Nuke viewer keeps its gain, so its card and its viewers
/// agree, it is saved and undone with the graph, and an 8-bit picture beside a linear frame stays
/// as it is. Refused: one switch for every viewer, which stretches a JPEG with the FITS beside
/// it; an Auto Stretch node before each preview, which puts a display choice into the pipeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayStretch {
    /// As the samples are.
    #[default]
    Linear,
    /// Through [`DisplayStretch::AUTO`], lumos's screen-transfer stretch.
    Auto,
}

impl DisplayStretch {
    /// Every stretch, in the picker's order.
    pub const ALL: [Self; 2] = [Self::Linear, Self::Auto];

    /// The stretch `Auto` applies: lumos's STF, each channel by its own statistics, the mode its
    /// [`ColorMode::PerChannel`] gives a quick screen preview, which also neutralizes a cast
    /// background as PixInsight's unlinked AutoSTF does.
    pub const AUTO: Stretch = Stretch {
        method: StretchMethod::AutoStf {
            shadow_sigmas: StretchMethod::AUTO_SHADOW_SIGMAS,
            target_bg: StretchMethod::AUTO_TARGET_BG,
        },
        color: ColorMode::PerChannel,
    };

    /// The name a document saves and a picker shows.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Auto => "Auto",
        }
    }
}

impl EnumVariants for DisplayStretch {
    fn variant_names() -> Vec<String> {
        Self::ALL.map(|stretch| stretch.name().to_owned()).to_vec()
    }
}

/// A name that is no stretch's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no display stretch has this name")]
pub struct UnknownDisplayStretch;

impl FromStr for DisplayStretch {
    type Err = UnknownDisplayStretch;

    /// One of [`DisplayStretch::name`]'s names.
    fn from_str(name: &str) -> Result<Self, UnknownDisplayStretch> {
        Self::ALL
            .into_iter()
            .find(|stretch| stretch.name() == name)
            .ok_or(UnknownDisplayStretch)
    }
}

/// Why an image has no stretched picture.
#[derive(Debug, thiserror::Error)]
pub enum DisplayStretchError {
    /// A side past what a `lumos` frame holds, which the stretch reads.
    #[error("the image is no frame the stretch reads")]
    Frame(#[source] FrameDescError),
    /// The stretch found no background to place: every pixel a fill, or one flat value.
    #[error("the auto stretch failed")]
    Stretch(#[source] OpError),
}

#[cfg(test)]
mod tests {
    use scenarium::EnumVariants;

    use crate::image::display_stretch::{DisplayStretch, UnknownDisplayStretch};

    /// Each stretch's name parses back to it, in the picker's order, and no other name parses.
    #[test]
    fn each_name_parses_back_to_its_stretch() {
        assert_eq!(DisplayStretch::variant_names(), ["Linear", "Auto"]);
        for stretch in DisplayStretch::ALL {
            assert_eq!(stretch.name().parse(), Ok(stretch));
        }
        assert_eq!("auto".parse::<DisplayStretch>(), Err(UnknownDisplayStretch));
        assert_eq!(DisplayStretch::default(), DisplayStretch::Linear);
    }
}
