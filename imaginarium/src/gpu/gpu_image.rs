use std::borrow::Cow;

use wgpu::util::DeviceExt;

use crate::prelude::*;

/// Wrapper for read-only buffer access.
#[derive(Debug)]
pub struct ReadBuffer<'a>(pub(crate) &'a wgpu::Buffer);

impl ReadBuffer<'_> {
    /// Returns the entire buffer as a binding resource.
    pub fn as_entire_binding(&self) -> wgpu::BindingResource<'_> {
        self.0.as_entire_binding()
    }
}

/// Wrapper for writable buffer access.
#[derive(Debug)]
pub struct WriteBuffer<'a>(pub(crate) &'a wgpu::Buffer);

impl WriteBuffer<'_> {
    /// Returns the entire buffer as a binding resource.
    pub fn as_entire_binding(&self) -> wgpu::BindingResource<'_> {
        self.0.as_entire_binding()
    }

    /// Returns a reference to the underlying buffer for queue operations.
    pub fn buffer(&self) -> &wgpu::Buffer {
        self.0
    }
}

/// Image data stored on the GPU as a buffer.
#[derive(Debug)]
pub struct GpuImage {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) desc: ImageDesc,
}

impl GpuImage {
    /// Creates a new GPU image from CPU image data.
    pub fn from_image(ctx: &Gpu, image: &Image) -> Self {
        let (bytes, desc): (Cow<[u8]>, ImageDesc) = if image.desc().is_aligned() {
            (Cow::Borrowed(image.bytes()), *image.desc())
        } else {
            let strided = image.clone().with_stride();
            (Cow::Owned(strided.bytes().to_vec()), *strided.desc())
        };

        let buffer = ctx
            .device()
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpu_image_buffer"),
                contents: &bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            });

        Self { buffer, desc }
    }

    /// Creates an empty GPU image with the given descriptor.
    pub fn new_empty(ctx: &Gpu, desc: ImageDesc) -> Self {
        let desc = desc.with_aligned_stride();
        let size = desc.size_in_bytes() as u64;

        let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self { buffer, desc }
    }

    /// Downloads GPU image data to CPU.
    pub fn to_image(&self, ctx: &Gpu) -> Result<Image> {
        let size = self.desc().size_in_bytes() as u64;

        let staging_buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_download_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &staging_buffer, 0, size);

        ctx.queue().submit(std::iter::once(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        ctx.device()
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();

        rx.recv()
            .expect("channel closed unexpectedly")
            .map_err(|e| Error::Gpu(format!("Failed to map buffer: {:?}", e)))?;

        let data = buffer_slice.get_mapped_range();
        let bytes = data.to_vec();

        drop(data);
        staging_buffer.unmap();

        Image::new_with_data(*self.desc(), bytes)
    }

    /// Creates a copy of this GPU image with a new buffer.
    pub fn clone_buffer(&self, ctx: &Gpu) -> Self {
        let size = self.desc().size_in_bytes() as u64;

        let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_clone_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &buffer, 0, size);

        ctx.queue().submit(std::iter::once(encoder.finish()));

        Self {
            buffer,
            desc: *self.desc(),
        }
    }

    /// Returns the image descriptor.
    pub fn desc(&self) -> &ImageDesc {
        &self.desc
    }

    /// Returns a read-only buffer wrapper for binding in shaders.
    pub fn read_buffer(&self) -> ReadBuffer<'_> {
        ReadBuffer(&self.buffer)
    }

    /// Returns a writable buffer wrapper for binding in shaders.
    ///
    /// Note: `&mut self` is intentional to prevent accidental writes to non-mutable buffers.
    pub fn write_buffer(&mut self) -> WriteBuffer<'_> {
        WriteBuffer(&self.buffer)
    }
}
