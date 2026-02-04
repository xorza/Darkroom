//! Image preparation stage: grayscale conversion, defect correction, CFA filter.

use crate::AstroImage;
use crate::common::Buffer2;
use crate::star_detection::buffer_pool::BufferPool;
use crate::star_detection::defect_map::DefectMap;
use crate::star_detection::median_filter::median_filter_3x3;

/// Convert an input image to a grayscale f32 buffer, applying defect
/// correction and CFA median filtering as needed.
///
/// Steps:
///   1. RGB → luminance (or copy for grayscale).
///   2. Replace defective pixels with local medians (if defect map provided).
///   3. 3×3 median filter to suppress Bayer/X-Trans artifacts (if CFA).
///
/// The returned buffer is acquired from `pool`; the caller owns it.
pub(crate) fn prepare(
    image: &AstroImage,
    defects: Option<&DefectMap>,
    pool: &mut BufferPool,
) -> Buffer2<f32> {
    let mut pixels = pool.acquire_f32();
    image.into_grayscale_buffer(&mut pixels);

    let mut scratch = pool.acquire_f32();

    // Defect correction
    if let Some(defect_map) = defects
        && !defect_map.is_empty()
    {
        defect_map.apply(&pixels, &mut scratch);
        std::mem::swap(&mut pixels, &mut scratch);
    }

    // CFA median filter
    if image.metadata.is_cfa {
        median_filter_3x3(&pixels, &mut scratch);
        std::mem::swap(&mut pixels, &mut scratch);
    }

    pool.release_f32(scratch);
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AstroImage, ImageDimensions};

    #[test]
    fn test_prepare_uniform() {
        let dim = ImageDimensions::new(64, 64, 1);
        let data = vec![0.5f32; 64 * 64];
        let image = AstroImage::from_pixels(dim, data);

        let mut pool = BufferPool::new(64, 64);
        let result = prepare(&image, None, &mut pool);

        assert_eq!(result.width(), 64);
        assert_eq!(result.height(), 64);
        for &v in result.pixels() {
            assert!((v - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn test_prepare_with_star() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.1f32; width * height];
        // Add bright pixel (simulating a star)
        data[32 * width + 32] = 0.9;

        let dim = ImageDimensions::new(width, height, 1);
        let image = AstroImage::from_pixels(dim, data);

        let mut pool = BufferPool::new(width, height);
        let result = prepare(&image, None, &mut pool);

        // Star pixel should be preserved (no CFA, no defects)
        assert!((result[(32, 32)] - 0.9).abs() < 1e-6);
    }
}
