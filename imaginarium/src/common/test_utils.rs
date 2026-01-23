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

/// Ensures the test output directory exists. Safe to call multiple times.
pub fn ensure_test_output_dir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        std::fs::create_dir_all(format!("{}/test_output", workspace_root()))
            .expect("Failed to create test_output directory");
    });
}

/// Returns the path to a test output file.
pub fn test_output(name: &str) -> String {
    format!("{}/test_output/{}", workspace_root(), name)
}

/// Loads the lena test image as RGBA_U8 format (895x551).
/// The image is cached and cloned on each call to avoid repeated file I/O.
pub fn load_lena_rgba_u8_895x551() -> Image {
    static LENA: OnceLock<Image> = OnceLock::new();
    LENA.get_or_init(|| {
        Image::read_file(test_resource("lena_895x551.tiff"))
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
pub fn load_lena_rgba_u8_61x38() -> Image {
    static LENA_SMALL: OnceLock<Image> = OnceLock::new();
    LENA_SMALL
        .get_or_init(|| {
            Image::read_file(test_resource("lena_61x38.tiff"))
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

/// Creates a test image with a deterministic pattern based on seed.
/// For integer formats, uses a byte pattern. For float formats, uses normalized values.
pub fn create_test_image(format: ColorFormat, width: usize, height: usize, seed: usize) -> Image {
    use crate::image::ImageDesc;

    let desc = ImageDesc::new(width, height, format);
    let mut img = Image::new_black(desc).unwrap();

    match (format.channel_size, format.channel_type) {
        (ChannelSize::_32bit, ChannelType::Float) => {
            let floats: &mut [f32] = bytemuck::cast_slice_mut(img.bytes_mut());
            for (i, val) in floats.iter_mut().enumerate() {
                *val = ((i + seed) % 100) as f32 / 100.0;
            }
        }
        _ => {
            for (i, byte) in img.bytes_mut().iter_mut().enumerate() {
                *byte = ((i + seed) * 37 % 256) as u8;
            }
        }
    }
    img
}

/// Loads the lena test image as RGBA_F32 format (895x551).
/// The image is cached and cloned on each call to avoid repeated file I/O.
pub fn load_lena_rgba_f32_895x551() -> Image {
    static LENA_F32: OnceLock<Image> = OnceLock::new();
    LENA_F32
        .get_or_init(|| {
            Image::read_file(test_resource("lena_895x551.tiff"))
                .unwrap()
                .convert(ColorFormat::RGBA_F32)
                .unwrap()
        })
        .clone()
}

/// Loads a small lena test image as RGBA_F32 format (61x38).
/// The image is cached and cloned on each call to avoid repeated file I/O.
pub fn load_lena_rgba_f32_61x38() -> Image {
    static LENA_SMALL_F32: OnceLock<Image> = OnceLock::new();
    LENA_SMALL_F32
        .get_or_init(|| {
            Image::read_file(test_resource("lena_61x38.tiff"))
                .unwrap()
                .convert(ColorFormat::RGBA_F32)
                .unwrap()
        })
        .clone()
}

/// Creates a test image filled with a constant f32 value.
pub fn create_test_image_f32(
    format: ColorFormat,
    width: usize,
    height: usize,
    value: f32,
) -> Image {
    use crate::image::ImageDesc;

    let desc = ImageDesc::new(width, height, format);
    let mut img = Image::new_black(desc).unwrap();
    let bytes = img.bytes_mut();
    for chunk in bytes.chunks_exact_mut(4) {
        let val_bytes = value.to_le_bytes();
        chunk.copy_from_slice(&val_bytes);
    }
    img
}
