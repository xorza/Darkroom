use atomic_refcell::{AtomicRef, AtomicRefCell, AtomicRefMut};

use super::ProcessingContext;
use crate::gpu::GpuImage;
use crate::prelude::*;

/// Storage location for image data.
#[derive(Debug)]
pub enum Storage {
    /// Image data is on the CPU.
    Cpu(Image),
    /// Image data is on the GPU.
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
    desc: ImageDesc,
    storage: AtomicRefCell<Option<Storage>>,
}

impl ImageBuffer {
    /// Creates a new ImageBuffer from a CPU image.
    pub fn from_cpu(image: Image) -> Self {
        let image = image.with_stride();
        Self {
            desc: *image.desc(),
            storage: AtomicRefCell::new(Some(Storage::Cpu(image))),
        }
    }

    /// Creates a new ImageBuffer from a GPU image.
    pub fn from_gpu(image: GpuImage) -> Self {
        Self {
            desc: *image.desc(),
            storage: AtomicRefCell::new(Some(Storage::Gpu(image))),
        }
    }

    /// Creates an empty ImageBuffer with no storage, just a descriptor.
    pub fn new_empty(desc: ImageDesc) -> Self {
        Self {
            desc: desc.with_aligned_stride(),
            storage: AtomicRefCell::new(None),
        }
    }

    /// Returns true if the image is currently on the GPU.
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

    /// Returns the image descriptor.
    pub fn desc(&self) -> &ImageDesc {
        &self.desc
    }

    /// Converts to GPU storage in place, uploading from CPU if needed.
    /// Allocates GPU storage if empty.
    /// Returns an immutable reference to the GPU image.
    pub fn make_gpu(&self, ctx: &ProcessingContext) -> Result<AtomicRef<'_, GpuImage>> {
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
    pub fn make_gpu_mut(&mut self, ctx: &ProcessingContext) -> Result<AtomicRefMut<'_, GpuImage>> {
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
                Some(Storage::Gpu(gpu_img)) => {
                    let gpu_ctx = ctx.gpu().expect("GPU image exists but no GPU context");
                    Storage::Cpu(gpu_img.to_image(gpu_ctx)?)
                }
                Some(Storage::Cpu(_)) => unreachable!(),
                None => Storage::Cpu(Image::new_black(self.desc)?),
            });
        }
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
        match self.storage.into_inner() {
            Some(Storage::Cpu(img)) => Ok(img),
            Some(Storage::Gpu(gpu_img)) => {
                let gpu_ctx = ctx.gpu().expect("GPU image exists but no GPU context");
                gpu_img.to_image_async(gpu_ctx).await
            }
            None => Image::new_black(self.desc),
        }
    }

    /// Consumes self and returns the GPU image, uploading from CPU if needed.
    /// Allocates GPU storage if empty.
    pub fn to_gpu(self, ctx: &ProcessingContext) -> Result<GpuImage> {
        self.ensure_gpu(ctx)?;
        match self.storage.into_inner() {
            Some(Storage::Gpu(img)) => Ok(img),
            _ => unreachable!(),
        }
    }

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

impl From<GpuImage> for ImageBuffer {
    fn from(image: GpuImage) -> Self {
        Self::from_gpu(image)
    }
}
