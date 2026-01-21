use atomic_refcell::{AtomicRef, AtomicRefCell, AtomicRefMut};

use super::ProcessingContext;
use crate::gpu::{Gpu, GpuImage, PendingDownload};
use crate::prelude::*;

/// A pending download that will store the result back into an ImageBuffer.
pub struct PendingBufferDownload<'a> {
    buffer: &'a ImageBuffer,
    download: PendingDownload,
}

impl<'a> PendingBufferDownload<'a> {
    /// Waits for this download to complete and stores the result back into the buffer.
    /// Assumes the GPU has already been polled to completion.
    fn wait(self, ctx: &Gpu) -> Result<()> {
        let image = PendingDownload::wait_all(std::iter::once(self.download), ctx)?
            .pop()
            .unwrap();
        self.buffer.set_cpu(image);
        Ok(())
    }

    /// Waits for all pending downloads and stores results back into their buffers.
    pub fn wait_all(downloads: Vec<PendingBufferDownload<'a>>, ctx: &Gpu) -> Result<()> {
        if downloads.is_empty() {
            return Ok(());
        }

        ctx.wait();
        for download in downloads {
            download.wait(ctx)?;
        }

        Ok(())
    }

    /// Waits for all pending downloads asynchronously and stores results back into their buffers.
    pub async fn wait_all_async(
        downloads: Vec<PendingBufferDownload<'a>>,
        ctx: &Gpu,
    ) -> Result<()> {
        if downloads.is_empty() {
            return Ok(());
        }

        ctx.wait_async().await;
        for download in downloads {
            download.wait(ctx)?;
        }

        Ok(())
    }
}

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
        if !self.is_gpu() {
            let gpu_ctx = ctx.gpu().ok_or(Error::NoGpuContext)?;
            let old = self.storage.borrow_mut().take();
            match old {
                Some(Storage::Cpu(img)) => {
                    let gpu_img = GpuImage::from_image(gpu_ctx, &img);
                    *self.storage.borrow_mut() = Some(Storage::Gpu(gpu_img));
                }
                Some(Storage::Gpu(_)) => unreachable!(),
                None => {
                    let gpu_img = GpuImage::new_empty(gpu_ctx, self.desc);
                    *self.storage.borrow_mut() = Some(Storage::Gpu(gpu_img));
                }
            }
        }
        Ok(())
    }

    /// Internal helper to ensure storage is CPU.
    fn ensure_cpu(&self, ctx: &ProcessingContext) -> Result<()> {
        if !self.is_cpu() {
            let old = self.storage.borrow_mut().take();
            match old {
                Some(Storage::Gpu(gpu_img)) => {
                    let gpu_ctx = ctx.gpu().expect("GPU image exists but no GPU context");
                    let cpu_img = gpu_img.to_image(gpu_ctx)?;
                    *self.storage.borrow_mut() = Some(Storage::Cpu(cpu_img));
                }
                Some(Storage::Cpu(_)) => unreachable!(),
                None => {
                    let image = Image::new_empty(self.desc)?;
                    *self.storage.borrow_mut() = Some(Storage::Cpu(image));
                }
            }
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

    /// Starts a GPU download without waiting, returning a pending download handle.
    /// If already on CPU or empty, returns None.
    /// Use `PendingBufferDownload::wait_all` to complete multiple downloads at once.
    pub fn start_download(&self, ctx: &ProcessingContext) -> Option<PendingBufferDownload<'_>> {
        let storage = self.storage.borrow();
        match &*storage {
            Some(Storage::Gpu(gpu_img)) => {
                let gpu_ctx = ctx.gpu().expect("GPU image exists but no GPU context");
                Some(PendingBufferDownload {
                    buffer: self,
                    download: gpu_img.start_download(gpu_ctx),
                })
            }
            _ => None,
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

    /// Stores a CPU image, replacing any existing storage.
    fn set_cpu(&self, image: Image) {
        *self.storage.borrow_mut() = Some(Storage::Cpu(image));
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
