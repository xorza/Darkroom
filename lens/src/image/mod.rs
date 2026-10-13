//! The `image` domain — `imaginarium`-backed nodes and types. This module is
//! also the home of [`Image`], the scenarium [`CustomValue`] every image-carrying
//! port passes.

mod codec;
mod format;
pub(crate) mod nodes;

use std::any::Any;
use std::borrow::Cow;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::sync::Arc;

use lumos::LinearImage;
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
///
/// `PlanarCpu` is the far larger variant (a `LinearImage` carries its metadata and three plane
/// handles inline), so every image pays its size. Boxing it to even the variants out would trade
/// ~680 bytes of inline padding — once per graph value, beside megabytes of pixels — for a heap
/// allocation and an indirection on the astro path, which is the wrong way round.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing would trade inline padding for an indirection on the astro path"
)]
enum Pixels {
    /// One `f32` plane per channel: what the `lumos` astro ops read and write.
    PlanarCpu(LinearImage),
    /// Interleaved samples: what the `imaginarium` ops take.
    InterleavedCpu(imaginarium::Image),
}

impl Image {
    /// A required image input to process, named `name`.
    pub(crate) fn input(name: &str) -> FuncInput {
        FuncInput::required(name, IMAGE_DATA_TYPE).description("Image to process.")
    }

    /// The interleaved pixels of a required image input, owned: moved out
    /// when the value is their last holder and copied when it is not,
    /// repacked from planes either way if that is what they are.
    pub(crate) fn take_interleaved(value: DynamicValue) -> imaginarium::Image {
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

    /// Takes the interleaved form, repacking from planes if that is what this holds.
    fn into_interleaved(self) -> imaginarium::Image {
        match self.pixels {
            Pixels::InterleavedCpu(image) => image,
            Pixels::PlanarCpu(planar) => imaginarium::Image::from(&planar),
        }
    }

    /// Takes the planes, deinterleaving if that is what this holds. An image that is already planar
    /// is returned untouched and uncopied — the case an all-astro chain hits at every node.
    fn into_planar(self) -> LinearImage {
        match self.pixels {
            Pixels::PlanarCpu(planar) => planar,
            Pixels::InterleavedCpu(image) => LinearImage::from(&image),
        }
    }

    /// The interleaved form, borrowed when this image already holds it and repacked when it does
    /// not. For the readers that only ever get a `&Image` — document serialization and the
    /// previewer — where taking ownership is not on offer and copying an already-interleaved master
    /// would be pure waste.
    pub fn interleaved(&self) -> Cow<'_, imaginarium::Image> {
        match &self.pixels {
            Pixels::InterleavedCpu(image) => Cow::Borrowed(image),
            Pixels::PlanarCpu(planar) => Cow::Owned(imaginarium::Image::from(planar)),
        }
    }

    /// [`Self::into_planar`] for an image still shared with other consumers, which therefore has to
    /// be copied rather than taken.
    pub(crate) fn planar(&self) -> Cow<'_, LinearImage> {
        match &self.pixels {
            Pixels::PlanarCpu(planar) => Cow::Borrowed(planar),
            Pixels::InterleavedCpu(image) => Cow::Owned(LinearImage::from(image)),
        }
    }

    /// Dimensions and format, without repacking either way.
    pub(crate) fn desc(&self) -> imaginarium::ImageDesc {
        match &self.pixels {
            Pixels::InterleavedCpu(image) => image.desc(),
            Pixels::PlanarCpu(planar) => {
                imaginarium::ImageDesc::new(planar.width(), planar.height(), planar.color_format())
            }
        }
    }
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
            Pixels::PlanarCpu(planar) => planar.sample_count() * size_of::<f32>(),
            Pixels::InterleavedCpu(image) => image.bytes().len(),
        };
        RamUsage { cpu, gpu: 0 }
    }
}

impl Display for Image {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.desc())
    }
}

impl From<imaginarium::Image> for Image {
    fn from(image: imaginarium::Image) -> Self {
        Self {
            pixels: Pixels::InterleavedCpu(image),
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
