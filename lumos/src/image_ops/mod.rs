//! Run the display/processing ops on the crate's planar [`LinearImage`] — one `Buffer2<f32>` per
//! channel, the same storage stacking and star detection use.
//!
//! - ops with genuine per-channel 2D structure ([`crate::image_ops::denoise`]'s wavelets,
//!   [`crate::image_ops::background_extraction`]'s surface fit) take their plane straight from the
//!   image and need no adapter at all;
//! - per-pixel ops read the planes in step: [`map_samples`] for work that treats every sample
//!   alike, [`map_rgb`] for work that needs a whole pixel at once (SCNR, the colour-preserving
//!   stretch), and the intensity-domain remap ([`remap_intensity`]) for the display enhancers.
//!
//! The submodules below are the image operations themselves (each an op-named config struct with an
//! in-place `apply`), plus their shared support: [`op`] (the `OpError` contract) and [`wavelet`]
//! (the multiscale primitive `denoise`/`hdr` build on).

#[cfg(all(test, feature = "real-data"))]
mod bench;

#[cfg(test)]
mod mem_budget_probe;

pub(crate) mod background_extraction;
pub(crate) mod color_calibration;
pub(crate) mod denoise;
pub(crate) mod hdr;
pub(crate) mod local_contrast;
#[cfg(feature = "ml")]
pub(crate) mod ml;
pub(crate) mod op;
pub(crate) mod rgb;
pub(crate) mod stretching;
pub(crate) mod wavelet;

use crate::image_ops::rgb::Rgb;
use crate::io::image::linear::LinearImage;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::math::size2us::Size2us;

/// Samples per rayon work item. Parallelizing per sample drowns a cheap per-pixel op in rayon's
/// recursive split/join overhead (it dominated SCNR); a coarse block amortizes that while staying
/// load-balanced and letting the inner loop auto-vectorize.
pub(crate) const SAMPLES_PER_BLOCK: usize = 8192;

/// Per-sample parallel in-place map over every plane. For work that treats each sample alike
/// whatever channel it belongs to; [`map_rgb`] is the form for work that needs the whole pixel.
pub(crate) fn map_samples(image: &mut LinearImage, sample: impl Fn(f32) -> f32 + Sync) {
    for plane in image.planes_mut() {
        plane
            .pixels_mut()
            .par_chunks_mut(SAMPLES_PER_BLOCK)
            .for_each(|block| {
                for value in block {
                    *value = sample(*value);
                }
            });
    }
}

/// Per-pixel parallel in-place map that needs all three channels at once. A no-op on a grayscale
/// image, which has no cross-channel relationship for `rgb` to act on — every caller here (SCNR,
/// background neutralization, the colour-preserving stretch) is meaningless in mono and returned
/// early on it when the storage was interleaved.
pub(crate) fn map_rgb(image: &mut LinearImage, rgb: impl Fn(Rgb) -> Rgb + Sync) {
    if !image.is_rgb() {
        return;
    }
    let [r, g, b] = image.rgb_planes_mut();
    r.par_chunks_mut(SAMPLES_PER_BLOCK)
        .zip(g.par_chunks_mut(SAMPLES_PER_BLOCK))
        .zip(b.par_chunks_mut(SAMPLES_PER_BLOCK))
        .for_each(|((r, g), b)| {
            for ((r, g), b) in r.iter_mut().zip(g.iter_mut()).zip(b.iter_mut()) {
                let out = rgb(Rgb {
                    r: *r,
                    g: *g,
                    b: *b,
                });
                *r = out.r;
                *g = out.g;
                *b = out.b;
            }
        });
}

/// Per-pixel combined intensity as a plane: the channel itself for L, `(r+g+b)/3` for RGB.
pub(crate) fn intensity_plane(image: &LinearImage) -> Buffer2<f32> {
    let size = Size2us::new(image.width(), image.height());
    if !image.is_rgb() {
        return image.channel(0).clone();
    }
    let (r, g, b) = (
        image.channel(0).pixels(),
        image.channel(1).pixels(),
        image.channel(2).pixels(),
    );
    let mut intensity = vec![0.0f32; size.pixel_count()];
    intensity
        .par_iter_mut()
        .zip(r.par_iter())
        .zip(g.par_iter())
        .zip(b.par_iter())
        .for_each(|(((out, &r), &g), &b)| *out = Rgb { r, g, b }.intensity());
    Buffer2::new(size.width, size.height, intensity)
}

/// Enhance an image in its intensity (luminance) domain: take the combined intensity, transform it
/// with `map`, then rescale every channel hue-preservingly so the new intensity matches. The shape
/// shared by the display enhancers ([`crate::image_ops::hdr`], [`crate::image_ops::local_contrast`]).
pub(crate) fn remap_intensity(
    image: &mut LinearImage,
    map: impl FnOnce(&Buffer2<f32>) -> Buffer2<f32>,
) {
    let intensity = intensity_plane(image);
    let mapped = map(&intensity);
    apply_intensity_remap(image, &intensity, &mapped);
}

/// Hue-preserving intensity remap: scale each pixel's channels by `mapped/intensity`
/// (with a highlight cap so a channel can't clip past white and shift hue); L takes
/// `mapped` directly. Output clamped to `[0, 1]`. `intensity`/`mapped` must match the
/// image's dimensions.
fn apply_intensity_remap(image: &mut LinearImage, intensity: &Buffer2<f32>, mapped: &Buffer2<f32>) {
    if !image.is_rgb() {
        image
            .channel_mut(0)
            .pixels_mut()
            .par_iter_mut()
            .zip(mapped.pixels().par_iter())
            .for_each(|(p, &m)| *p = m.clamp(0.0, 1.0));
        return;
    }
    let [r, g, b] = image.rgb_planes_mut();
    r.par_iter_mut()
        .zip(g.par_iter_mut())
        .zip(b.par_iter_mut())
        .zip(intensity.pixels().par_iter())
        .zip(mapped.pixels().par_iter())
        .for_each(|((((r, g), b), &i), &m)| {
            if i <= 0.0 {
                return;
            }
            let gain = m / i;
            let (mut nr, mut ng, mut nb) = (*r * gain, *g * gain, *b * gain);
            let maxc = nr.max(ng).max(nb);
            if maxc > 1.0 {
                let s = 1.0 / maxc;
                nr *= s;
                ng *= s;
                nb *= s;
            }
            *r = nr.max(0.0);
            *g = ng.max(0.0);
            *b = nb.max(0.0);
        });
}

#[cfg(test)]
mod tests {
    use crate::image_ops::internals::{gray_image, rgb_image};
    use crate::image_ops::*;
    use crate::math::size2us::Size2us;

    #[test]
    fn map_rgb_maps_every_channel_of_a_pixel_and_skips_grayscale() {
        // 2x1 RGB: pixels (0.1,0.2,0.3) and (0.4,0.5,0.6).
        let mut image = rgb_image(
            Size2us::new(2, 1),
            vec![0.1, 0.4],
            vec![0.2, 0.5],
            vec![0.3, 0.6],
        );
        map_rgb(&mut image, |px| px.scale(2.0));
        assert_eq!(image.channel(0).pixels(), &[0.2, 0.8]);
        assert_eq!(image.channel(1).pixels(), &[0.4, 1.0]);
        assert_eq!(image.channel(2).pixels(), &[0.6, 1.2]);

        // Grayscale has no cross-channel relationship for `rgb` to act on, so it is left alone
        // rather than having the closure applied to (l, l, l).
        let mut gray = gray_image(Size2us::new(3, 1), vec![0.25, 0.5, 0.75]);
        map_rgb(&mut gray, |px| px.scale(2.0));
        assert_eq!(gray.channel(0).pixels(), &[0.25, 0.5, 0.75]);
    }

    #[test]
    fn map_samples_maps_every_plane() {
        let mut image = rgb_image(
            Size2us::new(2, 1),
            vec![0.1, 0.4],
            vec![0.2, 0.5],
            vec![0.3, 0.6],
        );
        map_samples(&mut image, |v| v + 1.0);
        assert_eq!(image.channel(0).pixels(), &[1.1, 1.4]);
        assert_eq!(image.channel(1).pixels(), &[1.2, 1.5]);
        assert_eq!(image.channel(2).pixels(), &[1.3, 1.6]);

        let mut gray = gray_image(Size2us::new(3, 1), vec![0.0, 0.25, 0.5]);
        map_samples(&mut gray, |v| v + 0.25);
        assert_eq!(gray.channel(0).pixels(), &[0.25, 0.5, 0.75]);
    }

    #[test]
    fn intensity_plane_is_channel_mean_for_rgb_and_identity_for_l() {
        // RGB: (0.3,0,0) → 0.1, (0.6,0.6,0.6) → 0.6 (mean; approx for the /3 rounding).
        let rgb = rgb_image(
            Size2us::new(2, 1),
            vec![0.3, 0.6],
            vec![0.0, 0.6],
            vec![0.0, 0.6],
        );
        let i = intensity_plane(&rgb);
        assert!((i.pixels()[0] - 0.1).abs() < 1e-6 && (i.pixels()[1] - 0.6).abs() < 1e-6);

        let l = gray_image(Size2us::new(2, 1), vec![0.2, 0.7]);
        assert_eq!(intensity_plane(&l).pixels(), &[0.2, 0.7]);
    }

    #[test]
    fn apply_intensity_remap_scales_rgb_hue_preservingly() {
        // One pixel (0.2,0.1,0.1), I = 0.4/3; double the mapped intensity → gain 2.
        let mut image = rgb_image(Size2us::new(1, 1), vec![0.2], vec![0.1], vec![0.1]);
        let intensity = intensity_plane(&image);
        let mapped = Buffer2::new(1, 1, vec![intensity.pixels()[0] * 2.0]);
        apply_intensity_remap(&mut image, &intensity, &mapped);
        // each channel x2, none exceeds 1 → no cap
        assert_eq!(image.channel(0).pixels(), &[0.4]);
        assert_eq!(image.channel(1).pixels(), &[0.2]);
        assert_eq!(image.channel(2).pixels(), &[0.2]);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::io::image::ImageDimensions;
    use crate::io::image::linear::LinearImage;
    use crate::math::size2us::Size2us;
    use imaginarium::Buffer2;

    pub(crate) fn channel_plane(image: &LinearImage, channel: usize) -> Buffer2<f32> {
        image.channel(channel).clone()
    }

    pub(crate) fn channel_samples(image: &LinearImage, channel: usize) -> Vec<f32> {
        image.channel(channel).pixels().to_vec()
    }

    pub(crate) fn gray_image(size: Size2us, pixels: Vec<f32>) -> LinearImage {
        LinearImage::from(Buffer2::new(size.width, size.height, pixels))
    }

    pub(crate) fn rgb_image(
        size: Size2us,
        red: Vec<f32>,
        green: Vec<f32>,
        blue: Vec<f32>,
    ) -> LinearImage {
        LinearImage::from_planar_channels(
            ImageDimensions::new((size.width, size.height), 3),
            [red, green, blue],
        )
    }

    pub(crate) fn mean(samples: &[f32]) -> f32 {
        assert!(!samples.is_empty());
        samples.iter().sum::<f32>() / samples.len() as f32
    }

    pub(crate) fn standard_deviation(samples: &[f32]) -> f32 {
        let mean = mean(samples);
        (samples
            .iter()
            .map(|&sample| (sample - mean) * (sample - mean))
            .sum::<f32>()
            / samples.len() as f32)
            .sqrt()
    }
}
