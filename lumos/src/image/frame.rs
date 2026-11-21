//! [`Frame`]: an image's samples and the three things it carries beside them, once for both
//! sample layouts, and [`FrameParts`], a frame taken apart.

use common::{Buffer2, USize2};
use imaginarium::PlanarDesc;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::flags::flag_counts::FlagCounts;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::frame_desc;
use crate::image::metadata::image_metadata::ImageMetadata;

/// An image's samples — planes in [`LinearImage`](crate::LinearImage), a sensor mosaic in
/// [`CfaImage`](crate::CfaImage) — with its metadata, its data-quality flags and the gain a flat
/// applied to it.
#[derive(Debug, Clone)]
pub struct Frame<S> {
    pub metadata: ImageMetadata,
    pub(super) samples: S,
    /// The data-quality flags of the pixels that carry any — see [`PixelFlags`]. The samples under
    /// [`QualityFlags::NO_DATA`](crate::QualityFlags::NO_DATA) are a finite fill, not data.
    pub(super) flags: Option<PixelFlags>,
    /// The gain a flat applied to each pixel's value and noise — see [`FrameGain`]: of the planes'
    /// channels, or of a mosaic's colors. `None` for a frame no flat divided, and for a master,
    /// which the combine made.
    pub(super) flat_gain: Option<FrameGain>,
}

/// A [`Frame`] taken apart: its samples and what it carries beside them, each moved, not copied.
#[derive(Debug)]
pub struct FrameParts<S> {
    pub samples: S,
    pub metadata: ImageMetadata,
    pub flags: Option<PixelFlags>,
    pub flat_gain: Option<FrameGain>,
}

/// The samples a [`Frame`] holds: a [`PlanarImage`](imaginarium::PlanarImage) or a
/// [`MosaicPlane`](crate::MosaicPlane), and no other.
pub trait Samples: sealed::Planes {}

impl<S: sealed::Planes> Samples for S {}

pub(crate) mod sealed {
    use std::fmt;

    use arrayvec::ArrayVec;
    use common::Buffer2;
    use imaginarium::PlanarDesc;

    use crate::image::cfa::cfa_type::CfaType;

    /// What the frame layer reads of a frame's samples, whatever their layout.
    #[expect(
        unnameable_types,
        reason = "the seal of `Samples`: nameable in the crate alone, so no other type implements it"
    )]
    pub trait Planes: Send + Sync + fmt::Debug + Sized {
        /// What the planes alone do not say: nothing for planes, the pattern for a mosaic.
        type Pattern: Copy + fmt::Debug + Send + Sync;

        fn desc(&self) -> PlanarDesc;

        fn channel(&self, channel: usize) -> &Buffer2<f32>;

        fn pattern(&self) -> Self::Pattern;

        /// The sensor pattern of an undemosaiced mosaic; `None` for planes.
        fn cfa_type(&self) -> Option<CfaType>;

        /// The pattern samples of this layout have where a record states `cfa_type`, the inverse
        /// of [`Self::cfa_type`]: `None` when the record is of the other layout.
        fn pattern_of(cfa_type: Option<CfaType>) -> Option<Self::Pattern>;

        /// The channels a flat gain over these samples covers: the planes', or the pattern's
        /// colors.
        fn gain_channels(&self) -> usize;

        fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;

        /// The samples of `planes` under `pattern`.
        ///
        /// # Panics
        /// When `planes` are not the count the layout holds.
        fn from_planes(planes: ArrayVec<Buffer2<f32>, 3>, pattern: Self::Pattern) -> Self;
    }
}

impl<S: Samples> Frame<S> {
    /// The frame of `samples` with `metadata`, carrying no flags and no flat gain.
    ///
    /// # Panics
    /// When the samples have a side past [`MAX_SIDE`](crate::frame_desc::MAX_SIDE).
    pub(crate) fn of(samples: S, metadata: ImageMetadata) -> Self {
        frame_desc::assert_frame(samples.desc());
        Self {
            metadata,
            samples,
            flags: None,
            flat_gain: None,
        }
    }

    pub fn desc(&self) -> PlanarDesc {
        self.samples.desc()
    }

    pub fn size(&self) -> USize2 {
        self.desc().size()
    }

    /// The plane of channel `c`: 0 is mono, R or the mosaic, 1 is G, 2 is B.
    pub fn channel(&self, c: usize) -> &Buffer2<f32> {
        self.samples.channel(c)
    }

    /// The sensor pattern of an undemosaiced mosaic; `None` for planes.
    pub(crate) fn sensor_pattern(&self) -> Option<CfaType> {
        self.samples.cfa_type()
    }

    /// Resident RAM held by this frame: its `f32` planes, its flag plane, and a flat gain it holds
    /// alone. A shared gain's owner counts it. Metadata is negligible against a plane.
    pub fn ram_bytes(&self) -> usize {
        self.desc().byte_len()
            + self.flags.as_ref().map_or(0, |flags| flags.bytes().len())
            + self.flat_gain.as_ref().map_or(0, FrameGain::own_ram_bytes)
    }

    /// The data-quality flags of the pixels; `None` when no pixel carries one.
    pub const fn flags(&self) -> Option<&PixelFlags> {
        self.flags.as_ref()
    }

    /// How many pixels hold each flag, for a frame that carries any.
    pub(crate) fn flag_counts(&self) -> Option<&FlagCounts> {
        self.flags.as_ref().map(PixelFlags::counts)
    }

    /// Replace the flags with `flags`, of these pixels.
    ///
    /// # Panics
    /// When `flags` are of another size.
    pub fn set_flags(&mut self, flags: Option<PixelFlags>) {
        assert!(
            flags
                .as_ref()
                .is_none_or(|flags| flags.size() == self.size()),
            "flags for another geometry"
        );
        self.flags = flags;
    }

    /// The flags, moved out.
    pub(crate) const fn take_flags(&mut self) -> Option<PixelFlags> {
        self.flags.take()
    }

    /// The gain a flat applied to the pixels, for a frame a flat divided.
    pub const fn flat_gain(&self) -> Option<&FrameGain> {
        self.flat_gain.as_ref()
    }

    /// Replace the flat gain with `gain`, which covers these pixels and their channels or colors.
    ///
    /// # Panics
    /// When `gain` covers another size or another channel count.
    pub fn set_flat_gain(&mut self, gain: Option<FrameGain>) {
        assert!(
            gain.as_ref()
                .is_none_or(|gain| gain.covers(self.size(), self.samples.gain_channels())),
            "an image's flat gain covers another image's pixels"
        );
        self.flat_gain = gain;
    }
}

impl<S: Samples> From<Frame<S>> for FrameParts<S> {
    fn from(frame: Frame<S>) -> Self {
        let Frame {
            metadata,
            samples,
            flags,
            flat_gain,
        } = frame;
        Self {
            samples,
            metadata,
            flags,
            flat_gain,
        }
    }
}

/// The frame `parts` make.
///
/// # Panics
/// When the samples have a side past [`MAX_SIDE`](crate::frame_desc::MAX_SIDE), or the flags or
/// the flat gain cover other pixels.
impl<S: Samples> From<FrameParts<S>> for Frame<S> {
    fn from(parts: FrameParts<S>) -> Self {
        let FrameParts {
            samples,
            metadata,
            flags,
            flat_gain,
        } = parts;
        let mut frame = Self::of(samples, metadata);
        frame.set_flags(flags);
        frame.set_flat_gain(flat_gain);
        frame
    }
}
