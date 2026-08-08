//! The `image` domain — `imaginarium`-backed nodes and types. This module is
//! also the home of [`Image`], the scenarium [`CustomValue`] every image-carrying
//! port passes.

mod codec;
mod context;
mod format;
pub(crate) mod nodes;

use std::any::Any;
use std::sync::{Arc, LazyLock};

use parking_lot::{MappedRwLockReadGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use lumos::LinearImage;
use scenarium::{CustomValue, DataType, RamUsage, TypeId};

pub static IMAGE_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2".into());

pub(crate) static IMAGE_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::Custom(*IMAGE_TYPE_ID));

/// An image on a graph edge, in whichever layout its producer had.
///
/// The two node domains want opposite storage: `imaginarium`'s ops (and every GPU path) take an
/// interleaved [`imaginarium::ImageBuffer`], while `lumos`'s astronomical ops take a planar
/// [`LinearImage`] — one `f32` plane per channel. Rather than pick one and make the other convert
/// at every node, an image carries whichever layout its producer produced and repacks only where an
/// edge actually crosses between the domains. A chain that stays in one domain never repacks.
///
/// The shape is `imaginarium::ImageBuffer`'s, which solves the same problem one level down for
/// CPU-vs-GPU residency: one enum behind an `RwLock`, `make_*` to convert in place and borrow,
/// `to_*` to convert and take. Conversion happens through `&self` because the image-domain nodes
/// borrow their input out of a shared `DynamicValue`, which is what the lock is for.
pub struct Image {
    pixels: RwLock<Pixels>,
}

/// The two layouts an [`Image`] can be in — one or the other, never both and never neither.
///
/// `Planar` is the far larger variant (a `LinearImage` carries its metadata and three plane
/// handles inline), so every image pays its size even when interleaved. Boxing it to even the
/// variants out would trade ~680 bytes of inline padding — once per graph value, beside megabytes
/// of pixels — for a heap allocation and an indirection on the astro path, which is the wrong way
/// round.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum Pixels {
    /// One `f32` plane per channel: what the `lumos` astro ops read and write.
    Planar(LinearImage),
    /// Interleaved samples, CPU- or GPU-resident: what the `imaginarium` ops and every GPU path
    /// take.
    Interleaved(imaginarium::ImageBuffer),
}

impl Image {
    /// Converts to interleaved storage in place, repacking from planes if needed.
    /// Returns an immutable reference to the buffer.
    pub fn make_interleaved(&self) -> MappedRwLockReadGuard<'_, imaginarium::ImageBuffer> {
        let pixels = self.pixels.read();
        if matches!(*pixels, Pixels::Interleaved(_)) {
            return RwLockReadGuard::map(pixels, |pixels| match pixels {
                Pixels::Interleaved(buffer) => buffer,
                _ => unreachable!(),
            });
        }
        drop(pixels);

        let mut pixels = self.pixels.write();
        Self::ensure_interleaved(&mut pixels);
        let pixels = RwLockWriteGuard::downgrade(pixels);
        RwLockReadGuard::map(pixels, |pixels| match pixels {
            Pixels::Interleaved(buffer) => buffer,
            _ => unreachable!(),
        })
    }

    /// Converts to interleaved storage in place, repacking from planes if needed.
    /// Returns a mutable reference to the buffer.
    ///
    /// Note: `&mut self` is intentional to prevent accidental writes to non-mutable images.
    pub fn make_interleaved_mut(&mut self) -> &mut imaginarium::ImageBuffer {
        let pixels = self.pixels.get_mut();
        Self::ensure_interleaved(pixels);
        match pixels {
            Pixels::Interleaved(buffer) => buffer,
            Pixels::Planar(_) => unreachable!(),
        }
    }

    /// Takes the interleaved buffer, repacking from planes if needed.
    pub fn to_interleaved(self) -> imaginarium::ImageBuffer {
        match self.pixels.into_inner() {
            Pixels::Interleaved(buffer) => buffer,
            Pixels::Planar(planar) => {
                imaginarium::ImageBuffer::from(imaginarium::Image::from(&planar))
            }
        }
    }

    /// Converts to planar storage in place, deinterleaving (and pulling back from the GPU) if
    /// needed. Returns an immutable reference to the planes.
    pub fn make_planar(
        &self,
        ctx: &imaginarium::ProcessingContext,
    ) -> imaginarium::Result<MappedRwLockReadGuard<'_, LinearImage>> {
        let pixels = self.pixels.read();
        if matches!(*pixels, Pixels::Planar(_)) {
            return Ok(RwLockReadGuard::map(pixels, |pixels| match pixels {
                Pixels::Planar(planar) => planar,
                _ => unreachable!(),
            }));
        }
        drop(pixels);

        let mut pixels = self.pixels.write();
        Self::ensure_planar(&mut pixels, ctx)?;
        let pixels = RwLockWriteGuard::downgrade(pixels);
        Ok(RwLockReadGuard::map(pixels, |pixels| match pixels {
            Pixels::Planar(planar) => planar,
            _ => unreachable!(),
        }))
    }

    /// Takes the planes, deinterleaving (and pulling back from the GPU) if needed. An image that is
    /// already planar is returned untouched and uncopied — the case an all-astro chain hits at
    /// every node.
    pub fn to_planar(
        self,
        ctx: &imaginarium::ProcessingContext,
    ) -> imaginarium::Result<LinearImage> {
        match self.pixels.into_inner() {
            Pixels::Planar(planar) => Ok(planar),
            Pixels::Interleaved(buffer) => Ok(LinearImage::from(&buffer.to_cpu(ctx)?)),
        }
    }

    /// Dimensions and format, without converting either way.
    pub fn desc(&self) -> imaginarium::ImageDesc {
        match &*self.pixels.read() {
            Pixels::Interleaved(buffer) => buffer.desc,
            Pixels::Planar(planar) => imaginarium::ImageDesc::new(
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

    fn ensure_interleaved(pixels: &mut Pixels) {
        // `imaginarium::Image::from` borrows the planes, so the old variant is simply dropped by
        // the assignment — no `Option::take` dance needed to move out of `&mut`.
        if let Pixels::Planar(planar) = pixels {
            *pixels = Pixels::Interleaved(imaginarium::ImageBuffer::from(
                imaginarium::Image::from(&*planar),
            ));
        }
    }

    fn ensure_planar(
        pixels: &mut Pixels,
        ctx: &imaginarium::ProcessingContext,
    ) -> imaginarium::Result<()> {
        if let Pixels::Interleaved(buffer) = pixels {
            // Bound to a local so the CPU-readback guard is released before `pixels` is reassigned.
            let planar = LinearImage::from(&*buffer.make_cpu(ctx)?);
            *pixels = Pixels::Planar(planar);
        }
        Ok(())
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("pixels", &*self.pixels.read())
            .finish_non_exhaustive()
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
        match &*self.pixels.read() {
            Pixels::Planar(planar) => RamUsage {
                cpu: planar.sample_count() * size_of::<f32>(),
                gpu: 0,
            },
            Pixels::Interleaved(buffer) => {
                let mem = buffer.memory_usage();
                RamUsage {
                    cpu: mem.cpu,
                    gpu: mem.gpu,
                }
            }
        }
    }
}

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.desc())
    }
}

impl From<imaginarium::ImageBuffer> for Image {
    fn from(buffer: imaginarium::ImageBuffer) -> Self {
        Self {
            pixels: RwLock::new(Pixels::Interleaved(buffer)),
        }
    }
}

impl From<imaginarium::Image> for Image {
    fn from(image: imaginarium::Image) -> Self {
        Self::from(imaginarium::ImageBuffer::from(image))
    }
}

impl From<LinearImage> for Image {
    fn from(planar: LinearImage) -> Self {
        Self {
            pixels: RwLock::new(Pixels::Planar(planar)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaginarium::ProcessingContext;

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

    fn is_planar(image: &Image) -> bool {
        matches!(&*image.pixels.read(), Pixels::Planar(_))
    }

    #[test]
    fn make_interleaved_repacks_in_place_and_preserves_pixel_order() {
        let image = Image::from(planar_master());
        assert!(is_planar(&image));

        // Reading dimensions is not a conversion.
        assert_eq!(
            image.desc(),
            imaginarium::ImageDesc::new(2, 1, imaginarium::ColorFormat::RGB_F32)
        );
        assert!(is_planar(&image));

        {
            let buffer = image.make_interleaved();
            let cpu = buffer.make_cpu(&ProcessingContext::cpu_only()).unwrap();
            assert_eq!(cpu.bytes(), interleaved_bytes());
        }

        // Converted *in place*, so a second read is free — and since the layout is one enum field,
        // the planes are gone rather than kept alongside.
        assert!(!is_planar(&image));
        assert_eq!(image.ram_bytes().cpu, interleaved_bytes().len());
    }

    #[test]
    fn make_planar_deinterleaves_in_place() {
        let image = Image::from(imaginarium::Image::from(&planar_master()));
        assert!(!is_planar(&image));
        {
            let planar = image.make_planar(&ProcessingContext::cpu_only()).unwrap();
            assert_eq!(planar.channel(0).pixels(), &[0.125, 0.5]);
            assert_eq!(planar.channel(1).pixels(), &[0.25, 0.625]);
            assert_eq!(planar.channel(2).pixels(), &[0.375, 0.75]);
        }
        assert!(is_planar(&image));
    }

    #[test]
    fn to_planar_moves_the_planes_of_an_already_planar_image() {
        // The astro-chain fast path: consecutive `lumos` nodes hand the same allocation along
        // rather than repacking at each one.
        let planar = planar_master();
        let planes = planar.channel(0).pixels().as_ptr();
        let out = Image::from(planar)
            .to_planar(&ProcessingContext::cpu_only())
            .unwrap();
        assert_eq!(out.channel(0).pixels().as_ptr(), planes);
    }

    #[test]
    fn to_interleaved_repacks_an_image_that_holds_planes() {
        let buffer = Image::from(planar_master()).to_interleaved();
        let cpu = buffer.make_cpu(&ProcessingContext::cpu_only()).unwrap();
        assert_eq!(cpu.bytes(), interleaved_bytes());
    }
}
