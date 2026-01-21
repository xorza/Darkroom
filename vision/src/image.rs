use std::ops::{Deref, DerefMut};
use std::sync::{LazyLock, RwLock, RwLockReadGuard};

use graph::context::ContextManager;
use graph::data::{CustomValue, DataType};
use imaginarium::{ColorFormat, ImageBuffer, ImageDesc, Transform, Vec2};

use crate::vision_ctx::{VISION_CTX_TYPE, VisionCtx};

pub static IMAGE_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::from_custom("a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2", "Image"));

const PREVIEW_SIZE: u32 = 256;

/// Wrapper around `imaginarium::ImageBuffer` that implements `CustomValue`.
pub struct Image {
    pub buffer: imaginarium::ImageBuffer,
    preview: RwLock<Option<imaginarium::Image>>,
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
            preview: RwLock::new(None),
        }
    }

    pub fn preview(&self) -> RwLockReadGuard<'_, Option<imaginarium::Image>> {
        self.preview.read().unwrap()
    }
}

impl CustomValue for Image {
    fn data_type(&self) -> DataType {
        IMAGE_DATA_TYPE.clone()
    }

    fn gen_preview(&self, ctx_manager: &mut ContextManager) {
        let desc = self.buffer.desc();
        let max_dim = desc.width.max(desc.height);

        let scale = if max_dim <= PREVIEW_SIZE {
            1.0
        } else {
            PREVIEW_SIZE as f32 / max_dim as f32
        };

        let new_width = (desc.width as f32 * scale).round() as u32;
        let new_height = (desc.height as f32 * scale).round() as u32;

        let vision_ctx = ctx_manager.get::<VisionCtx>(&VISION_CTX_TYPE);

        // First transform to same format but smaller size

        let preview_desc = ImageDesc::new(new_width, new_height, desc.color_format);

        let mut scaled_buffer = ImageBuffer::new_empty(preview_desc);

        let result = Transform::new().scale(Vec2::new(scale, scale)).execute(
            &mut vision_ctx.processing_ctx,
            &self.buffer,
            &mut scaled_buffer,
        );

        if let Err(e) = result {
            tracing::error!("Failed to scale preview: {}", e);
            return;
        }

        // Read from GPU and convert to RGBA_U8
        let scaled_cpu = match scaled_buffer.to_cpu(&vision_ctx.processing_ctx) {
            Ok(img) => img,
            Err(e) => {
                tracing::error!("Failed to read preview from GPU: {}", e);
                return;
            }
        };

        let preview_image = match scaled_cpu.convert(ColorFormat::RGBA_U8) {
            Ok(img) => img,
            Err(e) => {
                tracing::error!("Failed to convert preview to RGBA_U8: {}", e);
                return;
            }
        };

        *self.preview.write().unwrap() = Some(preview_image);
    }
}

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.buffer.desc())
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
        &mut self.buffer
    }
}
