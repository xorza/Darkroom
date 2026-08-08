//! The `image` domain — `imaginarium`-backed nodes and types. This module is
//! also the home of [`Image`], the scenarium [`CustomValue`] every image-carrying
//! port passes.

mod codec;
mod format;
pub(crate) mod nodes;

use std::any::Any;
use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

use lumos::LinearImage;
use scenarium::{CustomValue, DataType, RamUsage, TypeId};

pub static IMAGE_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2".into());

pub(crate) static IMAGE_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::Custom(*IMAGE_TYPE_ID));

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
#[allow(clippy::large_enum_variant)]
enum Pixels {
    /// One `f32` plane per channel: what the `lumos` astro ops read and write.
    PlanarCpu(LinearImage),
    /// Interleaved samples: what the `imaginarium` ops take.
    InterleavedCpu(imaginarium::Image),
}

impl Image {
    /// Takes the interleaved form, repacking from planes if that is what this holds.
    pub fn to_interleaved(self) -> imaginarium::Image {
        match self.pixels {
            Pixels::InterleavedCpu(image) => image,
            Pixels::PlanarCpu(planar) => imaginarium::Image::from(&planar),
        }
    }

    /// Takes the planes, deinterleaving if that is what this holds. An image that is already planar
    /// is returned untouched and uncopied — the case an all-astro chain hits at every node.
    pub fn to_planar(self) -> LinearImage {
        match self.pixels {
            Pixels::PlanarCpu(planar) => planar,
            Pixels::InterleavedCpu(image) => LinearImage::from(&image),
        }
    }

    /// The interleaved form for in-place mutation, repacking this image if it holds planes.
    pub fn interleaved_mut(&mut self) -> &mut imaginarium::Image {
        if let Pixels::PlanarCpu(planar) = &self.pixels {
            self.pixels = Pixels::InterleavedCpu(imaginarium::Image::from(planar));
        }
        match &mut self.pixels {
            Pixels::InterleavedCpu(image) => image,
            Pixels::PlanarCpu(_) => unreachable!("the planar case was just repacked"),
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

    /// [`Self::to_planar`] for an image still shared with other consumers, which therefore has to be
    /// copied rather than taken.
    pub fn planar(&self) -> Cow<'_, LinearImage> {
        match &self.pixels {
            Pixels::PlanarCpu(planar) => Cow::Borrowed(planar),
            Pixels::InterleavedCpu(image) => Cow::Owned(LinearImage::from(image)),
        }
    }

    /// Dimensions and format, without repacking either way.
    pub fn desc(&self) -> imaginarium::ImageDesc {
        match &self.pixels {
            Pixels::InterleavedCpu(image) => image.desc(),
            Pixels::PlanarCpu(planar) => imaginarium::ImageDesc::new(
                planar.width(),
                planar.height(),
                if planar.is_rgb() {
                    imaginarium::ColorFormat::RGB_F32
                } else {
                    imaginarium::ColorFormat::L_F32
                },
            ),
        }
    }
}

impl CustomValue for Image {
    fn type_id(&self) -> TypeId {
        *IMAGE_TYPE_ID
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

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
mod tests {
    use super::*;

    /// 2x1 RGB whose planes are (0.125, 0.5), (0.25, 0.625), (0.375, 0.75) — dyadic, so the
    /// round-trip between layouts is exact.
    fn planar_master() -> LinearImage {
        LinearImage::from_planar_channels(
            lumos::ImageDimensions::new((2, 1), 3),
            [vec![0.125f32, 0.5], vec![0.25, 0.625], vec![0.375, 0.75]],
        )
    }

    /// The interleaved samples `planar_master` repacks to, in pixel order.
    fn interleaved_bytes() -> Vec<u8> {
        [0.125f32, 0.25, 0.375, 0.5, 0.625, 0.75]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    #[test]
    fn to_planar_moves_the_planes_of_an_already_planar_image() {
        // The astro-chain fast path: consecutive `lumos` nodes hand the same allocation along
        // rather than repacking at each one.
        let planar = planar_master();
        let planes = planar.channel(0).pixels().as_ptr();
        let out = Image::from(planar).to_planar();
        assert_eq!(out.channel(0).pixels().as_ptr(), planes);
    }

    #[test]
    fn crossing_to_the_interleaved_domain_repacks_in_pixel_order() {
        let image = Image::from(planar_master());
        // Reading dimensions is not a repack.
        assert_eq!(
            image.desc(),
            imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32)
        );
        assert_eq!(image.to_interleaved().bytes(), interleaved_bytes());
    }

    #[test]
    fn borrowing_is_free_in_the_matching_layout_and_repacks_in_the_other() {
        let interleaved = Image::from(
            imaginarium::Image::new_with_data(
                imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32),
                interleaved_bytes(),
            )
            .unwrap(),
        );
        assert!(matches!(interleaved.interleaved(), Cow::Borrowed(_)));
        assert!(matches!(interleaved.planar(), Cow::Owned(_)));
        assert_eq!(interleaved.planar().channel(1).pixels(), &[0.25, 0.625]);

        let planar = Image::from(planar_master());
        assert!(matches!(planar.planar(), Cow::Borrowed(_)));
        assert!(matches!(planar.interleaved(), Cow::Owned(_)));
    }

    #[test]
    fn mutating_repacks_the_image_itself() {
        let mut image = Image::from(planar_master());
        assert_eq!(image.interleaved_mut().desc().width, 2);
        assert!(matches!(image.pixels, Pixels::InterleavedCpu(_)));
    }
}
