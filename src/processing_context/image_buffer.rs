use atomic_refcell::{AtomicRef, AtomicRefCell, AtomicRefMut};

use super::ProcessingContext;
#[cfg(feature = "wgpu")]
use crate::common::error::Error;
use crate::common::error::Result;
#[cfg(feature = "wgpu")]
use crate::gpu::gpu_image::GpuImage;
use crate::image::{Image, ImageDesc};

/// Bytes an [`ImageBuffer`] currently holds, split by where they live: `cpu`
/// system RAM (a resident `Image`) vs `gpu` VRAM (a resident `GpuImage`). An
/// empty buffer (descriptor only) is zero on both — it holds no pixels yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImageMemory {
    pub cpu: usize,
    pub gpu: usize,
}

/// Storage location for image data.
#[derive(Debug)]
pub(crate) enum Storage {
    /// Image data is on the CPU.
    Cpu(Image),
    /// Image data is on the GPU.
    #[cfg(feature = "wgpu")]
    Gpu(GpuImage),
}

/// Smart image buffer that can live on CPU or GPU.
///
/// Transfers data between CPU and GPU in place as needed.
/// Can also be empty (no storage) while still having a descriptor.
/// Uses interior mutability to allow conversion between CPU/GPU storage
/// through shared references. Thread-safe via AtomicRefCell.
#[derive(Debug)]
pub struct ImageBuffer {
    pub desc: ImageDesc,
    storage: AtomicRefCell<Option<Storage>>,
}

impl ImageBuffer {
    /// Creates a new ImageBuffer from a CPU image.
    ///
    /// CPU storage is always tightly packed; any row-stride alignment a GPU
    /// backend needs is added by `GpuImage` on upload (and stripped on
    /// download), so it never leaks into the CPU representation.
    pub fn from_cpu(image: Image) -> Self {
        Self {
            desc: image.desc,
            storage: AtomicRefCell::new(Some(Storage::Cpu(image))),
        }
    }

    /// Creates a new ImageBuffer from a GPU image. The buffer's descriptor is
    /// the packed logical layout; the `GpuImage` keeps its own aligned stride.
    #[cfg(feature = "wgpu")]
    pub(crate) fn from_gpu(image: GpuImage) -> Self {
        let d = image.desc;
        Self {
            desc: ImageDesc::new(d.width, d.height, d.color_format),
            storage: AtomicRefCell::new(Some(Storage::Gpu(image))),
        }
    }

    /// Creates an empty ImageBuffer with no storage, just a (packed) descriptor.
    pub fn new_empty(desc: ImageDesc) -> Self {
        Self {
            desc: ImageDesc::new(desc.width, desc.height, desc.color_format),
            storage: AtomicRefCell::new(None),
        }
    }

    /// Returns true if the image is currently on the GPU.
    #[cfg(feature = "wgpu")]
    pub fn is_gpu(&self) -> bool {
        matches!(*self.storage.borrow(), Some(Storage::Gpu(_)))
    }

    /// Returns true if the image is currently on the CPU.
    pub fn is_cpu(&self) -> bool {
        matches!(*self.storage.borrow(), Some(Storage::Cpu(_)))
    }

    /// Returns true if the buffer has no storage allocated.
    pub fn is_empty(&self) -> bool {
        self.storage.borrow().is_none()
    }

    /// The bytes this buffer currently holds, split into CPU RAM vs GPU VRAM by
    /// its residency. Reflects *actual* allocation: an empty buffer is zero, a
    /// CPU-resident one counts its packed pixel bytes, a GPU-resident one its
    /// (round-to-4) buffer bytes. A buffer counts on one side only — the two are
    /// never resident at once.
    pub fn memory_usage(&self) -> ImageMemory {
        match &*self.storage.borrow() {
            None => ImageMemory::default(),
            Some(Storage::Cpu(img)) => ImageMemory {
                cpu: img.bytes().len(),
                gpu: 0,
            },
            #[cfg(feature = "wgpu")]
            Some(Storage::Gpu(img)) => ImageMemory {
                cpu: 0,
                gpu: img.buffer_size() as usize,
            },
        }
    }

    /// Converts to GPU storage in place, uploading from CPU if needed.
    /// Allocates GPU storage if empty.
    /// Returns an immutable reference to the GPU image.
    #[cfg(feature = "wgpu")]
    pub(crate) fn make_gpu(&self, ctx: &ProcessingContext) -> Result<AtomicRef<'_, GpuImage>> {
        self.ensure_gpu(ctx)?;
        let storage = self.storage.borrow();
        Ok(AtomicRef::map(storage, |s| match s {
            Some(Storage::Gpu(img)) => img,
            _ => unreachable!(),
        }))
    }

    /// Converts to GPU storage in place, uploading from CPU if needed.
    /// Allocates GPU storage if empty.
    /// Returns a mutable reference to the GPU image.
    ///
    /// Note: `&mut self` is intentional to prevent accidental writes to non-mutable buffers.
    #[cfg(feature = "wgpu")]
    pub(crate) fn make_gpu_mut(
        &mut self,
        ctx: &ProcessingContext,
    ) -> Result<AtomicRefMut<'_, GpuImage>> {
        self.ensure_gpu(ctx)?;
        let storage = self.storage.borrow_mut();
        Ok(AtomicRefMut::map(storage, |s| match s {
            Some(Storage::Gpu(img)) => img,
            _ => unreachable!(),
        }))
    }

    /// Converts to CPU storage in place, downloading from GPU if needed.
    /// Allocates CPU storage if empty.
    /// Returns an immutable reference to the CPU image.
    pub fn make_cpu(&self, ctx: &ProcessingContext) -> Result<AtomicRef<'_, Image>> {
        self.ensure_cpu(ctx)?;
        let storage = self.storage.borrow();
        Ok(AtomicRef::map(storage, |s| match s {
            Some(Storage::Cpu(img)) => img,
            _ => unreachable!(),
        }))
    }

    /// Converts to CPU storage in place, downloading from GPU if needed.
    /// Allocates CPU storage if empty.
    /// Returns a mutable reference to the CPU image.
    ///
    /// Note: `&mut self` is intentional to prevent accidental writes to non-mutable buffers.
    pub fn make_cpu_mut(&mut self, ctx: &ProcessingContext) -> Result<AtomicRefMut<'_, Image>> {
        self.ensure_cpu(ctx)?;
        let storage = self.storage.borrow_mut();
        Ok(AtomicRefMut::map(storage, |s| match s {
            Some(Storage::Cpu(img)) => img,
            _ => unreachable!(),
        }))
    }

    /// Internal helper to ensure storage is GPU.
    #[cfg(feature = "wgpu")]
    fn ensure_gpu(&self, ctx: &ProcessingContext) -> Result<()> {
        let mut storage = self.storage.borrow_mut();
        if !matches!(*storage, Some(Storage::Gpu(_))) {
            let gpu_ctx = ctx.gpu().ok_or(Error::NoGpuContext)?;
            *storage = Some(match storage.take() {
                Some(Storage::Cpu(img)) => Storage::Gpu(GpuImage::from_image(gpu_ctx, &img)),
                Some(Storage::Gpu(_)) => unreachable!(),
                None => Storage::Gpu(GpuImage::new_empty(gpu_ctx, self.desc)),
            });
        }
        Ok(())
    }

    /// Internal helper to ensure storage is CPU.
    fn ensure_cpu(&self, ctx: &ProcessingContext) -> Result<()> {
        let mut storage = self.storage.borrow_mut();
        if !matches!(*storage, Some(Storage::Cpu(_))) {
            *storage = Some(match storage.take() {
                #[cfg(feature = "wgpu")]
                Some(Storage::Gpu(gpu_img)) => {
                    let gpu_ctx = ctx.gpu().expect("GPU image exists but no GPU context");
                    Storage::Cpu(gpu_img.to_image(gpu_ctx)?)
                }
                Some(Storage::Cpu(_)) => unreachable!(),
                None => Storage::Cpu(Image::new_black(self.desc)?),
            });
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = ctx;
        Ok(())
    }

    /// Consumes self and returns the CPU image, downloading from GPU if needed.
    /// Allocates CPU storage if empty.
    pub fn to_cpu(self, ctx: &ProcessingContext) -> Result<Image> {
        self.ensure_cpu(ctx)?;
        match self.storage.into_inner() {
            Some(Storage::Cpu(img)) => Ok(img),
            _ => unreachable!(),
        }
    }

    /// Consumes self and returns the CPU image asynchronously, downloading from GPU if needed.
    /// Allocates CPU storage if empty.
    ///
    /// Note: Requires the GPU device to be polled (via `ctx.gpu().wait()` or similar)
    /// for the download to complete. The polling can happen from another thread.
    pub async fn to_cpu_async(self, ctx: &ProcessingContext) -> Result<Image> {
        #[cfg(not(feature = "wgpu"))]
        let _ = ctx;
        match self.storage.into_inner() {
            Some(Storage::Cpu(img)) => Ok(img),
            #[cfg(feature = "wgpu")]
            Some(Storage::Gpu(gpu_img)) => {
                let gpu_ctx = ctx.gpu().expect("GPU image exists but no GPU context");
                gpu_img.to_image_async(gpu_ctx).await
            }
            None => Image::new_black(self.desc),
        }
    }

    /// Consumes self and returns the GPU image, uploading from CPU if needed.
    /// Allocates GPU storage if empty.
    #[cfg(feature = "wgpu")]
    pub fn to_gpu(self, ctx: &ProcessingContext) -> Result<GpuImage> {
        self.ensure_gpu(ctx)?;
        match self.storage.into_inner() {
            Some(Storage::Gpu(img)) => Ok(img),
            _ => unreachable!(),
        }
    }

    #[cfg(feature = "wgpu")]
    pub fn as_gpu(self) -> Option<GpuImage> {
        match self.storage.into_inner() {
            Some(Storage::Gpu(img)) => Some(img),
            _ => None,
        }
    }
}

impl From<Image> for ImageBuffer {
    fn from(image: Image) -> Self {
        Self::from_cpu(image)
    }
}

#[cfg(feature = "wgpu")]
impl From<GpuImage> for ImageBuffer {
    fn from(image: GpuImage) -> Self {
        Self::from_gpu(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::color_format::ColorFormat;

    #[test]
    fn memory_usage_reflects_residency() {
        // A 4×3 RGBA_U8 image is tightly packed: 4 * 3 * 4 = 48 bytes.
        let desc = ImageDesc::new(4, 3, ColorFormat::RGBA_U8);
        assert_eq!(desc.size_in_bytes(), 48);

        // An empty buffer holds no pixels — zero on both sides.
        let empty = ImageBuffer::new_empty(desc);
        assert_eq!(empty.memory_usage(), ImageMemory::default());

        // CPU-resident: every byte is system RAM, none on the GPU.
        let cpu = ImageBuffer::from_cpu(Image::new_black(desc).unwrap());
        assert_eq!(cpu.memory_usage(), ImageMemory { cpu: 48, gpu: 0 });
    }
}
