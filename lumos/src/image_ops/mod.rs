//! Run the display/processing ops directly on imaginarium's interleaved `Image`.
//! These ops require a linear **f32** master (`L_F32` or `RGB_F32`):
//!
//! - per-pixel ops and per-channel ops with no 2D structure stay interleaved —
//!   [`par_map_pixels`] over the samples, an intensity-domain remap
//!   ([`remap_intensity`]), or a per-channel reduction/curve applied in place (e.g.
//!   [`crate::image_ops::color_calibration::neutralize_background`], per-channel
//!   [`crate::image_ops::stretching`]);
//! - ops with genuine per-channel 2D structure ([`crate::image_ops::denoise`]'s wavelets,
//!   [`crate::image_ops::background_extraction`]'s surface fit) stream one plane at a time through
//!   [`process_channels`]. The optional ML backend privately converts full images at its model
//!   boundary, where input and output buffers differ.
//!
//! The submodules below are the image operations themselves (each an op-named config struct with an
//! in-place `apply`), plus their shared support: [`op`] (the `OpError` contract) and [`wavelet`]
//! (the multiscale primitive `denoise`/`hdr` build on).

#[cfg(all(test, feature = "real-data"))]
mod bench;

pub(crate) mod background_extraction;
pub(crate) mod color_calibration;
pub(crate) mod denoise;
pub(crate) mod hdr;
pub(crate) mod local_contrast;
#[cfg(feature = "ml")]
pub(crate) mod ml;
pub(crate) mod op;
pub(crate) mod stretching;
pub(crate) mod wavelet;

use common::Rgb;
use imaginarium::{Buffer2, ChannelCount, Image};
use rayon::prelude::*;

use crate::image_ops::op::OpError;

/// Pixels per rayon work item. Parallelizing per pixel (`par_chunks_mut(3)`) drowns a cheap
/// per-pixel op in rayon's recursive split/join overhead (it dominated SCNR); a coarse block
/// amortizes that while staying load-balanced and letting the inner loop auto-vectorize.
pub(crate) const PIXELS_PER_BLOCK: usize = 8192;

/// Per-pixel parallel in-place map over an f32 master: `mono` for L, `rgb` for RGB. Parallel over
/// blocks of [`PIXELS_PER_BLOCK`] pixels, each block mapped serially.
pub(crate) fn par_map_pixels(
    image: &mut Image,
    mono: impl Fn(f32) -> f32 + Sync,
    rgb: impl Fn(Rgb) -> Rgb + Sync,
) {
    let is_rgb = image.desc.color_format.channel_count == ChannelCount::Rgb;
    let samples: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());
    if is_rgb {
        samples
            .par_chunks_mut(3 * PIXELS_PER_BLOCK)
            .for_each(|block| {
                for px in block.chunks_exact_mut(3) {
                    let out = rgb(Rgb {
                        r: px[0],
                        g: px[1],
                        b: px[2],
                    });
                    px[0] = out.r;
                    px[1] = out.g;
                    px[2] = out.b;
                }
            });
    } else {
        samples.par_chunks_mut(PIXELS_PER_BLOCK).for_each(|block| {
            for v in block {
                *v = mono(*v);
            }
        });
    }
}

/// Per-pixel combined intensity as a plane: the channel itself for L, `(r+g+b)/3`
/// for RGB.
pub(crate) fn intensity_plane(image: &Image) -> Buffer2<f32> {
    let (width, height) = (image.desc.width, image.desc.height);
    let samples: &[f32] = bytemuck::cast_slice(image.bytes());
    if image.desc.color_format.channel_count == ChannelCount::Rgb {
        let intensity = samples
            .chunks_exact(3)
            .map(|px| {
                Rgb {
                    r: px[0],
                    g: px[1],
                    b: px[2],
                }
                .intensity()
            })
            .collect();
        Buffer2::new(width, height, intensity)
    } else {
        Buffer2::new(width, height, samples.to_vec())
    }
}

/// Enhance an image in its intensity (luminance) domain: take the combined intensity, transform it
/// with `map`, then rescale every channel hue-preservingly so the new intensity matches. The shape
/// shared by the display enhancers ([`crate::image_ops::hdr`], [`crate::image_ops::local_contrast`]).
pub(crate) fn remap_intensity(image: &mut Image, map: impl FnOnce(&Buffer2<f32>) -> Buffer2<f32>) {
    let intensity = intensity_plane(image);
    let mapped = map(&intensity);
    apply_intensity_remap(image, &intensity, &mapped);
}

/// Hue-preserving intensity remap: scale each pixel's channels by `mapped/intensity`
/// (with a highlight cap so a channel can't clip past white and shift hue); L takes
/// `mapped` directly. Output clamped to `[0, 1]`. `intensity`/`mapped` must match the
/// image's dimensions.
fn apply_intensity_remap(image: &mut Image, intensity: &Buffer2<f32>, mapped: &Buffer2<f32>) {
    let is_rgb = image.desc.color_format.channel_count == ChannelCount::Rgb;
    let samples: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());
    if is_rgb {
        samples
            .par_chunks_mut(3)
            .zip(intensity.pixels().par_iter())
            .zip(mapped.pixels().par_iter())
            .for_each(|((px, &i), &m)| {
                if i <= 0.0 {
                    return;
                }
                let gain = m / i;
                let (mut nr, mut ng, mut nb) = (px[0] * gain, px[1] * gain, px[2] * gain);
                let maxc = nr.max(ng).max(nb);
                if maxc > 1.0 {
                    let s = 1.0 / maxc;
                    nr *= s;
                    ng *= s;
                    nb *= s;
                }
                px[0] = nr.max(0.0);
                px[1] = ng.max(0.0);
                px[2] = nb.max(0.0);
            });
    } else {
        samples
            .par_iter_mut()
            .zip(mapped.pixels().par_iter())
            .for_each(|(p, &m)| *p = m.clamp(0.0, 1.0));
    }
}

/// Run a per-channel operation that needs a contiguous 2D plane, **one channel at a
/// time**: gather the channel into a reused plane, process it, scatter it back. For the
/// spatial ops ([`crate::image_ops::denoise`], [`crate::image_ops::background_extraction`])
/// whose per-channel 2D work is cache-friendly on planar data. Streaming bounds the
/// planar scratch at a single plane whatever the channel count — a full RGB deinterleave
/// would hold three (~an extra full image resident).
pub(crate) fn process_channels(
    image: &mut Image,
    mut process: impl FnMut(&mut Buffer2<f32>) -> Result<(), OpError>,
) -> Result<(), OpError> {
    let channels = image.desc.color_format.channel_count.channel_count() as usize;
    let mut plane = Buffer2::new_default(image.desc.width, image.desc.height);
    for channel in 0..channels {
        gather_channel(image, channel, channels, &mut plane);
        process(&mut plane)?;
        scatter_channel(image, channel, channels, &plane);
    }
    Ok(())
}

/// Copy channel `channel` of the interleaved `image` into `plane`.
fn gather_channel(image: &Image, channel: usize, channels: usize, plane: &mut Buffer2<f32>) {
    let samples: &[f32] = bytemuck::cast_slice(image.bytes());
    plane
        .pixels_mut()
        .par_iter_mut()
        .zip(samples[channel..].par_iter().step_by(channels))
        .for_each(|(p, &s)| *p = s);
}

/// Write `plane` back as channel `channel` of the interleaved `image`.
fn scatter_channel(image: &mut Image, channel: usize, channels: usize, plane: &Buffer2<f32>) {
    let samples: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());
    samples[channel..]
        .par_iter_mut()
        .step_by(channels)
        .zip(plane.pixels().par_iter())
        .for_each(|(s, &p)| *s = p);
}

#[cfg(test)]
mod tests {
    use crate::image_ops::*;
    use imaginarium::{ColorFormat, ImageDesc};

    fn rgb_f32(width: usize, height: usize, samples: Vec<f32>) -> Image {
        Image::new_with_data(
            ImageDesc::new(width, height, ColorFormat::RGB_F32),
            bytemuck::cast_slice(&samples).to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn par_map_pixels_maps_rgb_per_pixel() {
        // 2x1 RGB: pixels (0.1,0.2,0.3) and (0.4,0.5,0.6).
        let mut image = rgb_f32(2, 1, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        par_map_pixels(&mut image, |l| l, |px| px.scale(2.0));
        let out: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(out, &[0.2, 0.4, 0.6, 0.8, 1.0, 1.2]);
    }

    #[test]
    fn par_map_pixels_maps_l_per_sample() {
        let mut image = Image::new_with_data(
            ImageDesc::new(3, 1, ColorFormat::L_F32),
            bytemuck::cast_slice(&[0.0f32, 0.25, 0.5]).to_vec(),
        )
        .unwrap();
        par_map_pixels(&mut image, |l| l + 0.25, |px| px);
        let out: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(out, &[0.25, 0.5, 0.75]);
    }

    #[test]
    fn intensity_plane_is_channel_mean_for_rgb_and_identity_for_l() {
        // RGB: (0.3,0,0) → 0.1, (0.6,0.6,0.6) → 0.6 (mean; approx for the /3 rounding).
        let rgb = rgb_f32(2, 1, vec![0.3, 0.0, 0.0, 0.6, 0.6, 0.6]);
        let i = intensity_plane(&rgb);
        assert!((i.pixels()[0] - 0.1).abs() < 1e-6 && (i.pixels()[1] - 0.6).abs() < 1e-6);

        let l = Image::new_with_data(
            ImageDesc::new(2, 1, ColorFormat::L_F32),
            bytemuck::cast_slice(&[0.2f32, 0.7]).to_vec(),
        )
        .unwrap();
        assert_eq!(intensity_plane(&l).pixels(), &[0.2, 0.7]);
    }

    #[test]
    fn apply_intensity_remap_scales_rgb_hue_preservingly() {
        // One pixel (0.2,0.1,0.1), I = 0.4/3; double the mapped intensity → gain 2.
        let mut image = rgb_f32(1, 1, vec![0.2, 0.1, 0.1]);
        let intensity = intensity_plane(&image);
        let mapped = Buffer2::new(1, 1, vec![intensity.pixels()[0] * 2.0]);
        apply_intensity_remap(&mut image, &intensity, &mapped);
        let out: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(out, &[0.4, 0.2, 0.2]); // each channel ×2, none exceeds 1 → no cap
    }

    #[test]
    fn process_channels_streams_each_channel_planar_and_in_order() {
        // Dyadic values so the +1.0 edit is exact in f32.
        let mut rgb = rgb_f32(2, 1, vec![0.125, 0.25, 0.375, 0.5, 0.625, 0.75]);
        let mut seen = Vec::new();
        process_channels(&mut rgb, |plane| {
            seen.push(plane.pixels().to_vec());
            for p in plane.pixels_mut() {
                *p += 1.0;
            }
            Ok(())
        })
        .unwrap();
        // Each channel arrived as its contiguous plane, R then G then B...
        assert_eq!(
            seen,
            [vec![0.125, 0.5], vec![0.25, 0.625], vec![0.375, 0.75]]
        );
        // ...and the edits landed back in the right interleaved slots.
        let out: &[f32] = bytemuck::cast_slice(rgb.bytes());
        assert_eq!(out, &[1.125, 1.25, 1.375, 1.5, 1.625, 1.75]);

        let mut l = Image::new_with_data(
            ImageDesc::new(3, 1, ColorFormat::L_F32),
            bytemuck::cast_slice(&[0.25f32, 0.5, 0.75]).to_vec(),
        )
        .unwrap();
        process_channels(&mut l, |plane| {
            for p in plane.pixels_mut() {
                *p *= 2.0;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(bytemuck::cast_slice::<u8, f32>(l.bytes()), &[0.5, 1.0, 1.5]);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::image_ops::gather_channel;
    use imaginarium::{Buffer2, DeinterleavedImageData, Image};

    pub(crate) fn channel_plane(image: &Image, channel: usize) -> Buffer2<f32> {
        let channels = image.desc.color_format.channel_count.channel_count() as usize;
        assert!(channel < channels);
        let mut plane = Buffer2::new_default(image.desc.width, image.desc.height);
        gather_channel(image, channel, channels, &mut plane);
        plane
    }

    pub(crate) fn channel_samples(image: &Image, channel: usize) -> Vec<f32> {
        channel_plane(image, channel).pixels().to_vec()
    }

    pub(crate) fn gray_image(width: usize, height: usize, pixels: Vec<f32>) -> Image {
        Image::from(&DeinterleavedImageData::from_channels([Buffer2::new(
            width, height, pixels,
        )]))
    }

    pub(crate) fn rgb_image(
        width: usize,
        height: usize,
        red: Vec<f32>,
        green: Vec<f32>,
        blue: Vec<f32>,
    ) -> Image {
        Image::from(&DeinterleavedImageData::from_channels([
            Buffer2::new(width, height, red),
            Buffer2::new(width, height, green),
            Buffer2::new(width, height, blue),
        ]))
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
