//! What the crate's own tests share: test images and image comparisons.

pub(crate) mod image_diff;

use std::io::BufReader;

use common::{Size2us, fs};

use crate::color::color_format::{ColorFormat, SampleType};
use crate::image::InterleavedImage;
use crate::image::image_desc::InterleavedDesc;
use crate::image::raster_format::RasterFormat;

/// A test image with a deterministic pattern based on seed: integer formats get a byte
/// pattern that reaches every byte value, float formats values on `[0, 1)`.
pub(crate) fn create_test_image(
    format: ColorFormat,
    size: Size2us,
    seed: usize,
) -> InterleavedImage {
    let mut img = InterleavedImage::new_black(InterleavedDesc::new(size, format)).unwrap();
    match format.sample_type {
        SampleType::F32 => {
            let floats: &mut [f32] = bytemuck::cast_slice_mut(img.bytes_mut());
            for (i, val) in floats.iter_mut().enumerate() {
                *val = ((i + seed) % 100) as f32 / 100.0;
            }
        }
        SampleType::U8 | SampleType::U16 => {
            for (i, byte) in img.bytes_mut().iter_mut().enumerate() {
                *byte = ((i + seed) * 37 % 256) as u8;
            }
        }
    }
    img
}

/// The 895×551 lena test image in `format`, read once and converted per call.
pub(crate) fn lena(format: ColorFormat) -> InterleavedImage {
    use std::sync::OnceLock;

    static LENA: OnceLock<InterleavedImage> = OnceLock::new();
    LENA.get_or_init(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_resources/lena_895x551.tiff"
        );
        InterleavedImage::decode(BufReader::new(fs::open(path).unwrap()), RasterFormat::Tiff)
            .unwrap()
    })
    .convert_to(format)
}

#[cfg(feature = "wgpu")]
pub(crate) mod gpu {
    use std::sync::OnceLock;

    use crate::gpu::Gpu;

    /// A shared GPU context for tests, created once: initialization takes about two seconds.
    ///
    /// `None` on a host without a usable adapter, which the first caller reports on stderr —
    /// a GPU test that returns early on `None` would otherwise pass without having run.
    pub(crate) fn test_gpu() -> Option<Gpu> {
        static TEST_GPU: OnceLock<Option<Gpu>> = OnceLock::new();
        TEST_GPU
            .get_or_init(|| match Gpu::new() {
                Ok(gpu) => Some(gpu),
                Err(error) => {
                    eprintln!("GPU tests skipped, no device: {error}");
                    None
                }
            })
            .clone()
    }
}
