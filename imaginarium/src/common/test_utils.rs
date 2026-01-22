use std::sync::OnceLock;

use crate::image::Image;
use crate::prelude::*;
use crate::processing_context::{GpuContext, ProcessingContext};

/// Returns the workspace root directory (parent of the crate directory).
fn workspace_root() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/..")
}

/// Returns the path to a test resource file relative to workspace root.
fn test_resource(name: &str) -> String {
    format!("{}/test_resources/{}", workspace_root(), name)
}

/// Loads the lena test image as RGBA_U8 format (895x551).
/// The image is cached and cloned on each call to avoid repeated file I/O.
pub fn load_lena_rgba_u8() -> Image {
    static LENA: OnceLock<Image> = OnceLock::new();
    LENA.get_or_init(|| {
        Image::read_file(test_resource("lena.tiff"))
            .unwrap()
            .convert(ColorFormat::from((
                ChannelCount::Rgba,
                ChannelSize::_8bit,
                ChannelType::UInt,
            )))
            .unwrap()
    })
    .clone()
}

/// Loads a small lena test image as RGBA_U8 format (61x38).
/// The image is cached and cloned on each call to avoid repeated file I/O.
pub fn load_lena_small_rgba_u8() -> Image {
    static LENA_SMALL: OnceLock<Image> = OnceLock::new();
    LENA_SMALL
        .get_or_init(|| {
            Image::read_file(test_resource("lena_small.tiff"))
                .unwrap()
                .convert(ColorFormat::from((
                    ChannelCount::Rgba,
                    ChannelSize::_8bit,
                    ChannelType::UInt,
                )))
                .unwrap()
        })
        .clone()
}

/// Returns a shared GPU context for tests.
/// This avoids the ~2 second initialization overhead per test.
pub fn test_gpu() -> Option<Gpu> {
    static TEST_GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    TEST_GPU.get_or_init(|| Gpu::new().ok()).clone()
}

/// Returns a shared ProcessingContext for tests.
/// This avoids the ~2 second GPU initialization overhead per test.
pub fn test_processing_context() -> ProcessingContext {
    match test_gpu() {
        Some(gpu) => ProcessingContext::with_gpu(GpuContext::new(gpu)),
        None => ProcessingContext::cpu_only(),
    }
}
