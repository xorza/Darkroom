//! Run the display/processing ops directly on imaginarium's interleaved `Image`,
//! instead of round-tripping through a planar `AstroImage`. These ops require a
//! linear **f32** master (`L_F32` or `RGB_F32`):
//!
//! - per-pixel ops use [`par_map_pixels`] on the interleaved samples — no
//!   deinterleave;
//! - the few genuinely per-channel/spatial ops ([`crate::denoise`],
//!   [`crate::background_extraction`]) [`deinterleave_f32`] to channel planes,
//!   process, and [`interleave_f32`] back.

use common::Rgb;
use imaginarium::{Buffer2, ChannelCount, ColorFormat, DeinterleavedImageData, Image};
use rayon::prelude::*;

/// The display ops are defined on a linear f32 master in L or RGB; anything else
/// (integer formats, RGBA) is a misuse here.
fn assert_f32_master(image: &Image) {
    let format = image.desc.color_format;
    assert!(
        format == ColorFormat::L_F32 || format == ColorFormat::RGB_F32,
        "display ops require an L_F32 or RGB_F32 image, got {format}"
    );
}

/// Per-pixel parallel in-place map over an f32 master: `mono` for L, `rgb` for RGB.
pub(crate) fn par_map_pixels(
    image: &mut Image,
    mono: impl Fn(f32) -> f32 + Sync,
    rgb: impl Fn(Rgb) -> Rgb + Sync,
) {
    assert_f32_master(image);
    let is_rgb = image.desc.color_format.channel_count == ChannelCount::Rgb;
    let samples: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());
    if is_rgb {
        samples.par_chunks_mut(3).for_each(|px| {
            let out = rgb(Rgb {
                r: px[0],
                g: px[1],
                b: px[2],
            });
            px[0] = out.r;
            px[1] = out.g;
            px[2] = out.b;
        });
    } else {
        samples.par_iter_mut().for_each(|v| *v = mono(*v));
    }
}

/// Per-pixel combined intensity as a plane: the channel itself for L, `(r+g+b)/3`
/// for RGB. The interleaved analogue of `AstroImage::intensity_plane`.
pub(crate) fn intensity_plane(image: &Image) -> Buffer2<f32> {
    assert_f32_master(image);
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

/// Hue-preserving intensity remap: scale each pixel's channels by `mapped/intensity`
/// (with a highlight cap so a channel can't clip past white and shift hue); L takes
/// `mapped` directly. Output clamped to `[0, 1]`. `intensity`/`mapped` must match the
/// image's dimensions.
pub(crate) fn apply_intensity_remap(
    image: &mut Image,
    intensity: &Buffer2<f32>,
    mapped: &Buffer2<f32>,
) {
    assert_f32_master(image);
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

/// Deinterleave an f32 master into its channel planes (1 for L, 3 for RGB).
pub(crate) fn deinterleave_f32(image: &Image) -> Vec<Buffer2<f32>> {
    assert_f32_master(image);
    match image.desc.color_format.channel_count {
        ChannelCount::L => {
            let planar: DeinterleavedImageData<1, f32> = image.try_into().unwrap();
            planar.channels.into_iter().collect()
        }
        ChannelCount::Rgb => {
            let planar: DeinterleavedImageData<3, f32> = image.try_into().unwrap();
            planar.channels.into_iter().collect()
        }
        ChannelCount::Rgba => unreachable!("assert_f32_master rejects RGBA"),
    }
}

/// Re-interleave channel planes (1 or 3) back into an f32 `Image`.
pub(crate) fn interleave_f32(planes: Vec<Buffer2<f32>>) -> Image {
    match planes.len() {
        1 => {
            let channels: [Buffer2<f32>; 1] = planes.try_into().unwrap();
            Image::from(&DeinterleavedImageData::from_channels(channels))
        }
        3 => {
            let channels: [Buffer2<f32>; 3] = planes.try_into().unwrap();
            Image::from(&DeinterleavedImageData::from_channels(channels))
        }
        n => panic!("interleave_f32 expects 1 (L) or 3 (RGB) planes, got {n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaginarium::ImageDesc;

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
    fn deinterleave_interleave_round_trips() {
        let image = rgb_f32(2, 1, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        let bytes = image.bytes().to_vec();
        let planes = deinterleave_f32(&image);
        assert_eq!(planes.len(), 3);
        assert_eq!(planes[0].pixels(), &[0.1, 0.4]); // R column
        let back = interleave_f32(planes);
        assert_eq!(back.bytes(), &bytes[..]);
    }
}
