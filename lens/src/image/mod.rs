//! The `image` domain — `imaginarium`-backed nodes and types. This module is
//! also the home of [`Image`], the scenarium [`CustomValue`] every image-carrying
//! port passes.

mod codec;
mod format;
pub(crate) mod nodes;

use std::any::Any;
use std::borrow::Cow;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

use common::USize2;
use imaginarium::{ChannelLayout, PlanarDesc, PlanarImage};
use lumos::{
    FrameDescError, FrameGain, ImageMetadata, LinearImage, LinearParts, PixelFlags, frame_desc,
};
use scenarium::{CustomValue, DataType, DynamicValue, FuncInput, RamUsage, TypeId};

pub const IMAGE_TYPE_ID: TypeId = TypeId::literal("a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2");

pub(crate) const IMAGE_DATA_TYPE: DataType = DataType::Custom(IMAGE_TYPE_ID);

/// An image on a graph wire, in whichever layout its producer had, with what it carries beside
/// its samples.
///
/// The two node domains want opposite storage: `imaginarium`'s ops take interleaved samples, while
/// `lumos`'s astronomical ops take a planar [`LinearImage`] — one `f32` plane per channel. Rather
/// than pick one and make the other convert at every node, an image carries whichever layout its
/// producer produced and repacks only where a wire crosses between the domains. A chain that stays
/// inside one domain never repacks.
///
/// The metadata, the flags and the flat gain describe the image, not its layout, so they sit
/// beside the samples in either layout and a repack keeps them. They cover the image's mono or RGB
/// planes: its size, and its channels without alpha. An `imaginarium` op takes the samples alone,
/// so its output carries none of them.
///
/// Repacking is a plain function of the value, not a hidden service: the conversion happens where a
/// node takes ownership of its input, so there is no interior mutability and no lock. Both layouts
/// are CPU-resident — nothing in `lens` initializes a GPU today. Device residency, when it is
/// wanted, belongs here as a third arm, with the upload/download and pipeline cache that
/// `imaginarium` exposes for exactly that.
#[derive(Debug)]
pub struct Image {
    pixels: Pixels,
    metadata: ImageMetadata,
    flags: Option<PixelFlags>,
    flat_gain: Option<FrameGain>,
}

/// The layouts an [`Image`]'s samples can be in — one or the other, never both and never neither.
#[derive(Debug)]
enum Pixels {
    /// One `f32` plane per channel: what the `lumos` astro ops read and write.
    Planar(PlanarImage),
    /// Interleaved samples in any of the nine formats: what the `imaginarium` ops take.
    Interleaved(imaginarium::InterleavedImage),
}

impl Image {
    /// A required image input to process, named `name`.
    pub(crate) fn input(name: &str) -> FuncInput {
        FuncInput::required(name, IMAGE_DATA_TYPE).description("Image to process.")
    }

    /// The interleaved pixels of a required image input, owned: moved out
    /// when the value is their last holder and copied when it is not,
    /// repacked from planes either way if that is what they are.
    pub(crate) fn take_interleaved(value: DynamicValue) -> imaginarium::InterleavedImage {
        match value.into_custom::<Self>() {
            Ok(image) => image.into_interleaved(),
            Err(value) => value.required_custom::<Self>().interleaved().into_owned(),
        }
    }

    /// [`take_interleaved`](Self::take_interleaved)'s planar counterpart, with the metadata, the
    /// flags and the flat gain: an already-planar last holder is moved untouched, the case an
    /// all-astro chain hits at every node. An image with a side past
    /// [`frame_desc::MAX_SIDE`] is no `lumos` frame, and is refused.
    pub(crate) fn take_planar(value: DynamicValue) -> Result<LinearImage, FrameDescError> {
        match value.into_custom::<Self>() {
            Ok(image) => image.into_planar(),
            Err(value) => value.required_custom::<Self>().to_planar(),
        }
    }

    /// Takes the interleaved form, repacking from planes if that is what this holds. The metadata,
    /// the flags and the flat gain stay behind: the `imaginarium` ops take samples alone.
    fn into_interleaved(self) -> imaginarium::InterleavedImage {
        match self.pixels {
            Pixels::Interleaved(image) => image,
            Pixels::Planar(planes) => imaginarium::InterleavedImage::from(planes),
        }
    }

    /// Takes the planes, deinterleaving if that is what this holds, with the metadata, the flags
    /// and the flat gain. Planes are moved into the image, not copied.
    fn into_planar(self) -> Result<LinearImage, FrameDescError> {
        frame_desc_of(self.desc())?;
        let pixels = match self.pixels {
            Pixels::Planar(planes) => planes,
            Pixels::Interleaved(image) => PlanarImage::without_alpha(&image),
        };
        Ok(LinearImage::from(LinearParts {
            pixels,
            metadata: self.metadata,
            flags: self.flags,
            flat_gain: self.flat_gain,
        }))
    }

    /// [`Self::into_planar`] for an image still shared with other consumers, which therefore has to
    /// be copied rather than taken.
    fn to_planar(&self) -> Result<LinearImage, FrameDescError> {
        frame_desc_of(self.desc())?;
        Ok(LinearImage::from(LinearParts {
            pixels: self.planar().into_owned(),
            metadata: self.metadata.clone(),
            flags: self.flags.clone(),
            flat_gain: self.flat_gain.clone(),
        }))
    }

    /// The interleaved samples, borrowed when this image already holds them and repacked when it
    /// does not. For the readers that only ever get a `&Image` — the `imaginarium` nodes that
    /// write a new image, and the previewer — where taking ownership is not on offer and copying
    /// an already-interleaved image would be pure waste.
    pub fn interleaved(&self) -> Cow<'_, imaginarium::InterleavedImage> {
        match &self.pixels {
            Pixels::Interleaved(image) => Cow::Borrowed(image),
            Pixels::Planar(planes) => Cow::Owned(imaginarium::InterleavedImage::from(planes)),
        }
    }

    /// [`Self::interleaved`]'s planar counterpart: the planes, borrowed when this image already
    /// holds them and deinterleaved, an alpha channel dropped, when it does not.
    pub(crate) fn planar(&self) -> Cow<'_, PlanarImage> {
        match &self.pixels {
            Pixels::Planar(planes) => Cow::Borrowed(planes),
            Pixels::Interleaved(image) => Cow::Owned(PlanarImage::without_alpha(image)),
        }
    }

    /// The `RGBA_U8` preview of this image at `size` — see [`imaginarium::Preview`] — read from
    /// the pixels where they lie, planes without interleaving them first.
    pub fn preview_rgba8(&self, size: USize2) -> imaginarium::InterleavedImage {
        let preview = imaginarium::Preview::new(size);
        match &self.pixels {
            Pixels::Interleaved(image) => preview.to_rgba8(image),
            Pixels::Planar(planes) => preview.planes_to_rgba8(planes),
        }
    }

    /// Dimensions and format, without repacking either way.
    pub fn desc(&self) -> imaginarium::InterleavedDesc {
        match &self.pixels {
            Pixels::Interleaved(image) => image.desc(),
            Pixels::Planar(planes) => imaginarium::InterleavedDesc::from(planes.desc()),
        }
    }
}

/// The descriptor of the `lumos` frame an image of `desc` makes, or why it makes none: a side
/// past [`frame_desc::MAX_SIDE`].
fn frame_desc_of(desc: imaginarium::InterleavedDesc) -> Result<PlanarDesc, FrameDescError> {
    frame_desc::checked(desc.size(), plane_layout(desc.color_format()))
}

/// The planes an image of `format` has, which its flags and its flat gain cover: mono or RGB, an
/// alpha channel left out.
fn plane_layout(format: imaginarium::ColorFormat) -> ChannelLayout {
    ChannelLayout::try_from(format.without_alpha().channel_count.count())
        .expect("a format without alpha is mono or RGB")
}

impl CustomValue for Image {
    fn type_id(&self) -> TypeId {
        IMAGE_TYPE_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    /// The samples, the flag plane and a flat gain the image holds alone, the same sum in either
    /// layout. Metadata is negligible against a plane.
    fn ram_bytes(&self) -> RamUsage {
        let samples = match &self.pixels {
            Pixels::Planar(planes) => planes.desc().byte_len(),
            Pixels::Interleaved(image) => image.bytes().len(),
        };
        let cpu = samples
            + self.flags.as_ref().map_or(0, |flags| flags.bytes().len())
            + self.flat_gain.as_ref().map_or(0, FrameGain::own_ram_bytes);
        RamUsage { cpu, gpu: 0 }
    }
}

impl Display for Image {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.desc())
    }
}

/// Interleaved samples, which carry no metadata, flags or flat gain.
impl From<imaginarium::InterleavedImage> for Image {
    fn from(image: imaginarium::InterleavedImage) -> Self {
        Self {
            pixels: Pixels::Interleaved(image),
            metadata: ImageMetadata::default(),
            flags: None,
            flat_gain: None,
        }
    }
}

/// Planes, which carry no metadata, flags or flat gain, in the layout the `lumos` nodes publish.
impl From<PlanarImage> for Image {
    fn from(pixels: PlanarImage) -> Self {
        Self {
            pixels: Pixels::Planar(pixels),
            metadata: ImageMetadata::default(),
            flags: None,
            flat_gain: None,
        }
    }
}

/// The planes and what they carry beside them, each moved.
impl From<LinearImage> for Image {
    fn from(image: LinearImage) -> Self {
        let LinearParts {
            pixels,
            metadata,
            flags,
            flat_gain,
        } = LinearParts::from(image);
        Self {
            pixels: Pixels::Planar(pixels),
            metadata,
            flags,
            flat_gain,
        }
    }
}

#[cfg(test)]
mod tests;
