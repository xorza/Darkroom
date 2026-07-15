//! The `image` domain — `imaginarium`-backed nodes and types. This module is
//! also the home of [`Image`] (a [`imaginarium::ImageBuffer`] wrapped as a
//! scenarium [`CustomValue`]).

mod blend_mode;
pub(crate) mod codec;
mod conversion_format;
pub(crate) mod library;
pub(crate) mod vision_ctx;

use std::any::Any;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock};

use scenarium::{CustomValue, DataType, RamUsage, TypeId};

pub static IMAGE_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2".into());

pub static IMAGE_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| DataType::Custom(*IMAGE_TYPE_ID));

/// Wrapper around `imaginarium::ImageBuffer` that implements `CustomValue`.
pub struct Image {
    pub buffer: imaginarium::ImageBuffer,
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
        Self { buffer }
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
        let mem = self.buffer.memory_usage();
        RamUsage {
            cpu: mem.cpu,
            gpu: mem.gpu,
        }
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
        &mut self.buffer
    }
}
