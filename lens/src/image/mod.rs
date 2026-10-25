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

use common::Size2us;
use imaginarium::{ChannelLayout, PlanarDesc, PlanarImage};
use lumos::{ImageMetadata, LinearImage};
use scenarium::{CustomValue, DataType, DynamicValue, FuncInput, RamUsage, TypeId};

pub const IMAGE_TYPE_ID: TypeId = TypeId::literal("a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2");

pub(crate) const IMAGE_DATA_TYPE: DataType = DataType::Custom(IMAGE_TYPE_ID);

/// An image on a graph edge, in whichever layout its producer had.
///
/// The two node domains want opposite storage: `imaginarium`'s ops take interleaved samples, while
/// `lumos`'s astronomical ops take a planar [`LinearImage`] — one `f32` plane per channel. Rather
/// than pick one and make the other convert at every node, an image carries whichever layout its
/// producer produced and repacks only where an edge actually crosses between the domains. A chain
/// that stays inside one domain never repacks.
///
/// Repacking is a plain function of the value, not a hidden service: the conversion happens where a
/// node takes ownership of its input, so there is no interior mutability and no lock. Both variants
/// are CPU-resident — nothing in `lens` initializes a GPU today. Device residency, when it is
/// wanted, belongs here as a third variant, with the upload/download and pipeline cache that
/// `imaginarium` exposes for exactly that.
#[derive(Debug)]
pub struct Image {
    pixels: Pixels,
}

/// The layouts an [`Image`] can be in — one or the other, never both and never neither.
#[derive(Debug)]
enum Pixels {
    /// One `f32` plane per channel: what the `lumos` astro ops read and write.
    PlanarCpu(LinearImage),
    /// Interleaved samples, what the `imaginarium` ops take, and what their decoder stated of them:
    /// default for an op's output, which states nothing.
    InterleavedCpu {
        image: imaginarium::InterleavedImage,
        metadata: ImageMetadata,
    },
}

impl Image {
    /// A planar image of `width × height`, in the layout the `lumos` nodes publish: one `f32`
    /// plane per channel, mono from one plane and RGB from three.
    ///
    /// # Panics
    /// When `planes` holds neither one nor three planes, or a plane does not hold
    /// `width × height` samples.
    pub fn from_planes(
        size: impl Into<Size2us>,
        planes: impl IntoIterator<Item = Vec<f32>>,
    ) -> Self {
        let planes: Vec<Vec<f32>> = planes.into_iter().collect();
        let layout =
            ChannelLayout::try_from(planes.len()).expect("an image has one plane or three");
        let desc = PlanarDesc::new(size, layout);
        Self::from(LinearImage::from_planar_channels(desc, planes))
    }

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

    /// [`take_interleaved`](Self::take_interleaved)'s planar counterpart: an
    /// already-planar last holder is moved untouched, the case an all-astro
    /// chain hits at every node.
    pub(crate) fn take_planar(value: DynamicValue) -> LinearImage {
        match value.into_custom::<Self>() {
            Ok(image) => image.into_planar(),
            Err(value) => value.required_custom::<Self>().planar().into_owned(),
        }
    }

    /// Takes the interleaved form, repacking from planes if that is what this holds. The metadata
    /// stays behind: the `imaginarium` ops take samples alone.
    fn into_interleaved(self) -> imaginarium::InterleavedImage {
        match self.pixels {
            Pixels::InterleavedCpu { image, .. } => image,
            Pixels::PlanarCpu(planar) => imaginarium::InterleavedImage::from(&planar),
        }
    }

    /// Takes the planes, deinterleaving if that is what this holds, with the metadata. An image
    /// that is already planar is returned untouched and uncopied — the case an all-astro chain hits
    /// at every node.
    fn into_planar(self) -> LinearImage {
        match self.pixels {
            Pixels::PlanarCpu(planar) => planar,
            Pixels::InterleavedCpu { image, metadata } => {
                let planes = PlanarImage::try_from(image.without_alpha());
                planes_of(planes.expect("an image without alpha has planes"), metadata)
            }
        }
    }

    /// The interleaved form, borrowed when this image already holds it and repacked when it does
    /// not. For the readers that only ever get a `&Image` — document serialization and the
    /// previewer — where taking ownership is not on offer and copying an already-interleaved master
    /// would be pure waste.
    pub fn interleaved(&self) -> Cow<'_, imaginarium::InterleavedImage> {
        match &self.pixels {
            Pixels::InterleavedCpu { image, .. } => Cow::Borrowed(image),
            Pixels::PlanarCpu(planar) => Cow::Owned(imaginarium::InterleavedImage::from(planar)),
        }
    }

    /// [`Self::into_planar`] for an image still shared with other consumers, which therefore has to
    /// be copied rather than taken.
    pub(crate) fn planar(&self) -> Cow<'_, LinearImage> {
        match &self.pixels {
            Pixels::PlanarCpu(planar) => Cow::Borrowed(planar),
            Pixels::InterleavedCpu { image, metadata } => {
                // The alpha drop takes the image, so one with alpha is copied first; one without
                // converts straight from the borrow.
                let planes = if image.desc().color_format().has_alpha() {
                    PlanarImage::try_from(image.clone().without_alpha())
                } else {
                    PlanarImage::try_from(image)
                };
                let planes = planes.expect("an image without alpha has planes");
                Cow::Owned(planes_of(planes, metadata.clone()))
            }
        }
    }

    /// Dimensions and format, without repacking either way.
    pub(crate) fn desc(&self) -> imaginarium::InterleavedDesc {
        match &self.pixels {
            Pixels::InterleavedCpu { image, .. } => image.desc(),
            Pixels::PlanarCpu(planar) => imaginarium::InterleavedDesc::from(planar.desc()),
        }
    }
}

/// The image of `planes`, carrying `metadata`.
fn planes_of(planes: PlanarImage, metadata: ImageMetadata) -> LinearImage {
    let mut planar = LinearImage::from(planes);
    planar.metadata = metadata;
    planar
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

    fn ram_bytes(&self) -> RamUsage {
        let cpu = match &self.pixels {
            Pixels::PlanarCpu(planar) => planar.ram_bytes(),
            Pixels::InterleavedCpu { image, .. } => image.bytes().len(),
        };
        RamUsage { cpu, gpu: 0 }
    }
}

impl Display for Image {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.desc())
    }
}

impl From<imaginarium::InterleavedImage> for Image {
    fn from(image: imaginarium::InterleavedImage) -> Self {
        Self {
            pixels: Pixels::InterleavedCpu {
                image,
                metadata: ImageMetadata::default(),
            },
        }
    }
}

impl From<LinearImage> for Image {
    fn from(planar: LinearImage) -> Self {
        Self {
            pixels: Pixels::PlanarCpu(planar),
        }
    }
}

#[cfg(test)]
mod tests;
