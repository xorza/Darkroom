use crate::common::color_format::{ColorFormat, SampleType};
use crate::image::Image;
use crate::image::image_desc::ImageDesc;

/// A test image with a deterministic pattern based on seed: integer formats get a byte
/// pattern that reaches every byte value, float formats values on `[0, 1)`.
pub(crate) fn create_test_image(
    format: ColorFormat,
    width: usize,
    height: usize,
    seed: usize,
) -> Image {
    let mut img = Image::new_black(ImageDesc::new(width, height, format)).unwrap();
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
#[cfg(test)]
pub(crate) fn lena(format: ColorFormat) -> Image {
    use std::sync::OnceLock;

    static LENA: OnceLock<Image> = OnceLock::new();
    LENA.get_or_init(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_resources/lena_895x551.tiff"
        );
        Image::read_file(path).unwrap()
    })
    .convert_to(format)
}

#[cfg(all(test, feature = "wgpu"))]
pub(crate) mod gpu {
    use std::sync::OnceLock;

    use crate::gpu::Gpu;

    /// A shared GPU context for tests, created once: initialization takes about two seconds.
    ///
    /// `None` on a host without a usable adapter, which the first caller reports on stderr —
    /// a GPU test that returns early on `None` would otherwise pass without having run.
    #[expect(
        clippy::print_stderr,
        reason = "the skip notice is what keeps a skipped GPU test from reading as a pass"
    )]
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
