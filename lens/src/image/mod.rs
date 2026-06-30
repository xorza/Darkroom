//! The `image` domain — `imaginarium`-backed nodes and types. This module is
//! also the home of [`Image`] (a [`imaginarium::ImageBuffer`] wrapped as a
//! scenarium [`CustomValue`] with a CPU-built thumbnail preview).

mod blend_mode;
pub(crate) mod codec;
mod conversion_format;
pub(crate) mod library;
pub(crate) mod vision_ctx;

use std::any::Any;
use std::ops::{Deref, DerefMut};
use std::sync::LazyLock;

use common::Slot;
use imaginarium::{ColorFormat, ImageBuffer, ImageDesc, Transform, Vec2};
use scenarium::context::ContextManager;
use scenarium::data::{CustomValue, DataType, PendingPreview, TypeId};

use crate::image::vision_ctx::{VISION_CTX_TYPE, VisionCtx};

pub static IMAGE_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2".into());

pub static IMAGE_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| DataType::Custom(*IMAGE_TYPE_ID));

const PREVIEW_SIZE: usize = 256;

/// Wrapper around `imaginarium::ImageBuffer` that implements `CustomValue`.
pub struct Image {
    pub buffer: imaginarium::ImageBuffer,
    preview: Slot<imaginarium::Image>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("buffer", &self.buffer)
            .finish_non_exhaustive()
    }
}

impl Image {
    pub fn new(buffer: imaginarium::ImageBuffer) -> Self {
        Self {
            buffer,
            preview: Slot::default(),
        }
    }

    pub fn take_preview(&self) -> Option<imaginarium::Image> {
        self.preview.take()
    }
}

impl CustomValue for Image {
    fn type_id(&self) -> TypeId {
        *IMAGE_TYPE_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn gen_preview(&self, ctx_manager: &mut ContextManager) -> Option<Box<dyn PendingPreview>> {
        let desc = self.buffer.desc;
        let max_dim = desc.width.max(desc.height);

        let scale = if max_dim <= PREVIEW_SIZE {
            1.0
        } else {
            PREVIEW_SIZE as f32 / max_dim as f32
        };

        let new_width = (desc.width as f32 * scale).round() as usize;
        let new_height = (desc.height as f32 * scale).round() as usize;

        let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

        let preview_desc = ImageDesc::new(new_width, new_height, desc.color_format);
        let mut scaled_buffer = ImageBuffer::new_empty(preview_desc);

        // CPU-only: the transform runs on the CPU and the result is already
        // CPU-resident, so the downscaled preview is built inline (a 256px frame
        // is cheap) with no GPU round-trip — hence no pending work to poll.
        if let Err(e) = Transform::new().scale(Vec2::new(scale, scale)).execute(
            &mut vision_ctx.processing_ctx,
            &self.buffer,
            &mut scaled_buffer,
        ) {
            tracing::error!("Failed to scale preview: {e}");
            return None;
        }

        let scaled_cpu = match scaled_buffer.to_cpu(&vision_ctx.processing_ctx) {
            Ok(img) => img,
            Err(e) => {
                tracing::error!("Failed to read preview: {e}");
                return None;
            }
        };

        match scaled_cpu.convert(ColorFormat::RGBA_U8) {
            Ok(preview_image) => self.preview.send(preview_image),
            Err(e) => tracing::error!("Failed to convert preview to RGBA_U8: {e}"),
        }

        None
    }
}

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.buffer.desc)
    }
}

impl From<imaginarium::ImageBuffer> for Image {
    fn from(buffer: imaginarium::ImageBuffer) -> Self {
        Image::new(buffer)
    }
}

impl From<imaginarium::Image> for Image {
    fn from(image: imaginarium::Image) -> Self {
        Image::new(imaginarium::ImageBuffer::from(image))
    }
}

impl Deref for Image {
    type Target = imaginarium::ImageBuffer;

    fn deref(&self) -> &Self::Target {
        &self.buffer
    }
}

impl DerefMut for Image {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.preview.take();
        &mut self.buffer
    }
}
