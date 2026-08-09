//! Debug image output for visual tests: PNG writers, tone mapping, and annotated overlays.
//!
//! These write to `test_output/` for a human to look at; nothing here is asserted on. The
//! grading a test actually asserts lives in [`report`] and
//! [`metrics`](crate::testing::synthetic::metrics).

pub(crate) mod comparison;
pub(crate) mod report;

use image::GrayImage;
use imaginarium::{ColorFormat, Image, ImageDesc};
use std::path::Path;

use crate::{
    math::size2us::Size2us, stacking::star_detection::star::Star,
    testing::synthetic::observe::ObservedSource,
    testing::visual::comparison::create_comparison_image,
};

/// Extension every debug image is written with.
const TEST_OUTPUT_IMAGE_EXT: &str = "png";

/// How an f32 plane is mapped into `[0, 1]` before being written as 8-bit.
///
/// Named `ToneMap` rather than `Stretch` because [`crate::Stretch`] is a published pipeline type
/// — this is display-only, and nothing here feeds the pipeline. It replaces plain/stretched
/// function pairs that were duplicated at three levels (`GrayImage`, imaginarium `Image`, and the
/// gallery's own byte writer), where adding a mapping meant editing three places.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ToneMap {
    /// Clamp `[0,1]`; shows true pixel levels.
    Clamp,
    /// Rescale min..max onto `[0,1]`; reveals structure inside a narrow range.
    AutoRange,
    /// asinh over min..max; reveals faint structure without flattening the highlights.
    Asinh,
}

impl ToneMap {
    /// Map `pixels` into `[0, 1]`.
    fn apply(self, pixels: &[f32]) -> Vec<f32> {
        match self {
            ToneMap::Clamp => pixels.iter().map(|&p| p.clamp(0.0, 1.0)).collect(),
            ToneMap::AutoRange => {
                let (lo, span) = Self::range(pixels);
                pixels.iter().map(|&p| (p - lo) / span).collect()
            }
            ToneMap::Asinh => {
                let (lo, span) = Self::range(pixels);
                // astropy-style AsinhStretch: y = asinh(x/a) / asinh(1/a), a = soft knee.
                let a = 0.1f32;
                let denom = (1.0 / a).asinh();
                pixels
                    .iter()
                    .map(|&p| {
                        let x = ((p - lo) / span).clamp(0.0, 1.0);
                        ((x / a).asinh() / denom).clamp(0.0, 1.0)
                    })
                    .collect()
            }
        }
    }

    /// Minimum and a non-zero span, for the two mappings that normalise against the data.
    fn range(pixels: &[f32]) -> (f32, f32) {
        let lo = pixels.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = pixels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (lo, (hi - lo).max(1e-10))
    }
}

/// Build an output path with the configured test image extension.
/// Takes a base path and replaces or adds the extension from `TEST_OUTPUT_IMAGE_EXT`.
pub(crate) fn output_path(base: &Path) -> std::path::PathBuf {
    base.with_extension(TEST_OUTPUT_IMAGE_EXT)
}

/// Convert an f32 grayscale plane to an imaginarium RGB_F32 image under `tone`.
pub(crate) fn gray_to_rgb(pixels: &[f32], size: Size2us, tone: ToneMap) -> Image {
    let desc = ImageDesc::new(size.width, size.height, ColorFormat::RGB_F32);
    let rgb: Vec<f32> = tone
        .apply(pixels)
        .into_iter()
        .flat_map(|v| [v, v, v])
        .collect();
    Image::new_with_data(desc, bytemuck::cast_slice(&rgb).to_vec()).unwrap()
}

/// Save imaginarium Image to file using the configured test output format.
/// Converts to RGB_U8 if needed since some formats don't support float data.
pub(crate) fn save_image(image: Image, path: &Path) {
    let out = output_path(path);
    let image_u8 = if image.desc().color_format.channel_type == imaginarium::ChannelType::Float {
        image.convert(ColorFormat::RGB_U8).unwrap()
    } else {
        image
    };
    image_u8.save_file(&out).expect("Failed to save image");
}

/// Convert an f32 plane to an 8-bit grayscale image under `tone`.
fn to_gray(pixels: &[f32], size: Size2us, tone: ToneMap) -> GrayImage {
    let bytes: Vec<u8> = tone
        .apply(pixels)
        .into_iter()
        .map(|v| (v * 255.0) as u8)
        .collect();
    GrayImage::from_raw(size.width as u32, size.height as u32, bytes).unwrap()
}

/// Convert boolean mask to grayscale image.
#[cfg(feature = "real-data")]
fn mask_to_gray(mask: &[bool], size: Size2us) -> GrayImage {
    let bytes: Vec<u8> = mask.iter().map(|&b| if b { 255 } else { 0 }).collect();
    GrayImage::from_raw(size.width as u32, size.height as u32, bytes).unwrap()
}

/// Convert labeled image to colored visualization.
///
/// Each label gets a unique color for easy visualization.
#[cfg(feature = "real-data")]
pub(crate) fn labels_to_rgb(labels: &imaginarium::Buffer2<u32>) -> image::RgbImage {
    use image::{Rgb, RgbImage};

    // Generate distinct colors for labels using golden ratio
    let label_to_color = |label: u32| -> Rgb<u8> {
        if label == 0 {
            return Rgb([0, 0, 0]);
        }
        let hue = ((label as f32) * 0.618_034) % 1.0;
        hsv_to_rgb(hue, 0.8, 0.9)
    };

    let pixels: Vec<u8> = labels
        .iter()
        .flat_map(|&l| {
            let Rgb([r, g, b]) = label_to_color(l);
            [r, g, b]
        })
        .collect();

    RgbImage::from_raw(labels.width() as u32, labels.height() as u32, pixels).unwrap()
}

/// Convert HSV to RGB color.
#[cfg(feature = "real-data")]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> image::Rgb<u8> {
    use image::Rgb;

    let c = v * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r, g, b) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    Rgb([
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ])
}

/// Write a whole `LinearImage` under `test_output/<name>`, letting imaginarium do the channel
/// conversion. The multi-channel counterpart to [`save`], which takes one f32 plane and tone-maps
/// it here; this one has colour to preserve and no single plane to map.
#[cfg(feature = "real-data")]
pub(crate) fn save_linear(image: &crate::io::image::linear::LinearImage, name: &str) {
    use common::internals::test_output_path;

    let path = test_output_path(name);
    std::fs::create_dir_all(path.parent().unwrap()).expect("create test_output dir");
    imaginarium::Image::from(image)
        .convert(ColorFormat::RGB_U8)
        .expect("convert to RGB_U8")
        .save_file(&path)
        .expect("save png");
    eprintln!("wrote {}", path.display());
}

/// Write an f32 plane as an 8-bit image under `tone`, with the configured extension.
pub(crate) fn save(pixels: &[f32], size: Size2us, path: &Path, tone: ToneMap) {
    to_gray(pixels, size, tone)
        .save(output_path(path))
        .expect("write debug image");
}

/// Save RGB image to file using the configured test output format.
#[cfg(feature = "real-data")]
pub(crate) fn save_rgb(image: &image::RgbImage, path: &Path) {
    let out = output_path(path);
    image.save(&out).expect("Failed to save RGB image");
}

/// Save comparison image showing ground truth vs detected stars.
pub(crate) fn save_comparison(
    pixels: &[f32],
    size: Size2us,
    ground_truth: &[ObservedSource],
    detected: &[Star],
    match_radius: f32,
    path: &Path,
) {
    let image = create_comparison_image(pixels, size, ground_truth, detected, match_radius);
    save_image(image, path);
}

/// Save mask to file using the configured test output format.
#[cfg(feature = "real-data")]
pub(crate) fn save_mask(mask: &[bool], size: Size2us, path: &Path) {
    let out = output_path(path);
    let img = mask_to_gray(mask, size);
    img.save(&out).expect("Failed to save mask image");
}

#[cfg(test)]
mod tests {
    use crate::math::size2us::Size2us;
    use crate::testing::visual::*;

    /// Each mapping over the same plane, so the three rules are pinned against one another.
    #[test]
    fn tone_maps_differ_on_the_same_plane() {
        let size = Size2us::new(2, 2);

        // Clamp: value x 255, truncated. 0.5 -> 127, 0.25 -> 63.
        let img = to_gray(&[0.0, 0.5, 1.0, 0.25], size, ToneMap::Clamp);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(1, 0).0[0], 127);
        assert_eq!(img.get_pixel(0, 1).0[0], 255);
        assert_eq!(img.get_pixel(1, 1).0[0], 63);

        // AutoRange: 0.2..0.8 rescaled onto 0..255, so the ends pin exactly and 0.4 lands a
        // third of the way up. That is 255/3 = 85 in exact arithmetic, but f32 puts the ratio a
        // hair under 1/3 and the cast truncates, so 84.
        let img = to_gray(&[0.2, 0.4, 0.6, 0.8], size, ToneMap::AutoRange);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(1, 0).0[0], 84);
        assert_eq!(img.get_pixel(1, 1).0[0], 255);

        // Asinh: same endpoints, but the knee lifts the midtones well above AutoRange.
        let img = to_gray(&[0.2, 0.4, 0.6, 0.8], size, ToneMap::Asinh);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(1, 1).0[0], 255);
        assert!(
            img.get_pixel(1, 0).0[0] > 84,
            "asinh must lift 0.4 above AutoRange's 84, got {}",
            img.get_pixel(1, 0).0[0]
        );
    }

    /// A flat plane has no range to stretch; the guarded span must not divide by zero.
    #[test]
    fn auto_range_survives_a_flat_plane() {
        let img = to_gray(&[0.5; 4], Size2us::new(2, 2), ToneMap::AutoRange);
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
    }
}
