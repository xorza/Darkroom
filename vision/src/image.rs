use std::any::Any;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock};

use common::SharedFn;
use graph::context::ContextManager;
use graph::data::{CustomValue, DataType, PreviewFn};

pub static IMAGE_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::from_custom("a69f9a9c-3be7-4d8b-abb1-dbd5c9ee4da2", "Image"));

pub static IMAGE_PREVIEW_FN: LazyLock<PreviewFn> = LazyLock::new(|| {
    SharedFn::new(Arc::new(
        |_data: &dyn Any, _ctx_manager: &mut ContextManager| {
            tracing::info!("Generating preview for Image");
        },
    ))
});

/// Wrapper around `imaginarium::ImageBuffer` that implements `CustomValue`.
#[derive(Debug)]
pub struct Image(pub imaginarium::ImageBuffer);

impl CustomValue for Image {
    fn data_type(&self) -> DataType {
        IMAGE_DATA_TYPE.clone()
    }

    fn preview_fn() -> PreviewFn {
        IMAGE_PREVIEW_FN.clone()
    }
}

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.desc())
    }
}

impl From<imaginarium::ImageBuffer> for Image {
    fn from(buffer: imaginarium::ImageBuffer) -> Self {
        Image(buffer)
    }
}

impl From<imaginarium::Image> for Image {
    fn from(image: imaginarium::Image) -> Self {
        Image(imaginarium::ImageBuffer::from(image))
    }
}

impl Deref for Image {
    type Target = imaginarium::ImageBuffer;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Image {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
