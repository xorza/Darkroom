use crate::image_ops::hdr::Hdr;
use crate::internals::assertions::assert_close_slice;
use crate::internals::images::{gray_image as gray, rgb_image as rgb};
use crate::internals::prelude::*;
use crate::math::wavelet::{atrous_smooth, max_scales};
use std::mem;

/// A smooth radial brightness dome — bright center (~1.0), dark corners (~0.1). The large-scale
/// brightness HDR is meant to compress.
fn dome(size: Size2us) -> Vec<f32> {
    let (cx, cy) = (
        (size.width as f32 - 1.0) / 2.0,
        (size.height as f32 - 1.0) / 2.0,
    );
    let sigma = size.width as f32 / 4.0;
    (0..size.pixel_count())
        .map(|i| {
            let p = size.point_of(i);
            let (x, y) = (p.x as f32, p.y as f32);
            let r2 = (x - cx) * (x - cx) + (y - cy) * (y - cy);
            0.1 + 0.9 * (-r2 / (2.0 * sigma * sigma)).exp()
        })
        .collect()
}

#[test]
fn hdr_amount_zero_is_identity() {
    let px = dome(Size2us::new(64, 64));
    let mut img = gray(Size2us::new(64, 64), px.clone());
    Hdr {
        scales: 3,
        amount: 0.0,
    }
    .apply(&mut img)
    .unwrap();
    assert_eq!(
        img.channel(0).to_vec(),
        px,
        "amount 0 leaves the image untouched"
    );
}

#[test]
fn hdr_compresses_large_scale_contrast() {
    // The dome lives in the residual (scales=3 → residual captures >8 px structure); compressing it
    // shrinks the center-vs-corner contrast while keeping it monotone.
    let size = Size2us::new(128, 128);
    let px = dome(size);
    let ci = size.index_of(Vec2us::new(size.width / 2, size.height / 2));
    let in_contrast = px[ci] - px[0];
    let mut img = gray(size, px);
    Hdr {
        scales: 3,
        amount: 0.5,
    }
    .apply(&mut img)
    .unwrap();
    let out = img.channel(0).to_vec();
    let out_contrast = out[ci] - out[0];
    assert!(
        out_contrast < in_contrast * 0.7,
        "large-scale contrast compressed: {out_contrast} < {in_contrast}"
    );
    assert!(out_contrast > 0.0, "center stays brighter than the corner");
}

#[test]
fn hdr_amount_controls_compression() {
    let size = Size2us::new(128, 128);
    let px = dome(size);
    let ci = size.index_of(Vec2us::new(size.width / 2, size.height / 2));
    let contrast_at = |amount: f32| {
        let mut img = gray(size, px.clone());
        Hdr { scales: 3, amount }.apply(&mut img).unwrap();
        let o = img.channel(0).to_vec();
        o[ci] - o[0]
    };
    assert!(
        contrast_at(0.8) < contrast_at(0.3),
        "more amount = more compression"
    );
}

#[test]
fn hdr_preserves_fine_detail() {
    // Dome + a 1-px ±0.03 checkerboard texture: the dome (residual) compresses, the texture (finest
    // detail layer) is preserved.
    let size = Size2us::new(128, 128);
    let mut px = dome(size);
    for (i, p) in px.iter_mut().enumerate() {
        *p += if (i % size.width + i / size.width).is_multiple_of(2) {
            0.03
        } else {
            -0.03
        };
    }
    let mut img = gray(size, px.clone());
    Hdr {
        scales: 3,
        amount: 0.6,
    }
    .apply(&mut img)
    .unwrap();
    let out = img.channel(0).to_vec();
    // Adjacent-pixel contrast along a dark row (corner side, no clipping) is the fine texture.
    let tex_in: f32 = (0..size.width - 1).map(|x| (px[x + 1] - px[x]).abs()).sum();
    let tex_out: f32 = (0..size.width - 1)
        .map(|x| (out[x + 1] - out[x]).abs())
        .sum();
    assert!(
        tex_out > 0.5 * tex_in,
        "fine detail preserved: {tex_out} vs {tex_in}"
    );
}

/// The starlet residual of `plane` over `scales`, as `hdr_map` smooths it.
fn residual(plane: &[f32], size: Size2us, scales: usize) -> Vec<f32> {
    let mut c_curr = Buffer2::new(size.width, size.height, plane.to_vec());
    let mut c_next = Buffer2::new_default(size.width, size.height);
    let mut tmp = Buffer2::new_default(size.width, size.height);
    for j in 0..scales {
        atrous_smooth(&c_curr, &mut c_next, &mut tmp, 1 << j);
        mem::swap(&mut c_curr, &mut c_next);
    }
    c_curr.pixels().to_vec()
}

/// What compressing a linear base by subtraction gives: `I − amount·(residual − mean)`, the
/// approach the log domain replaces.
fn linear_hdr(px: &[f32], size: Size2us, scales: usize, amount: f32) -> Vec<f32> {
    let residual = residual(px, size, scales);
    let mean = (residual.iter().map(|&v| f64::from(v)).sum::<f64>() / residual.len() as f64) as f32;
    px.iter()
        .zip(&residual)
        .map(|(&i, &r)| i - amount * (r - mean))
        .collect()
}

/// The literal reference: materialize every detail layer of the log intensity, flatten the
/// residual toward its mean, re-sum and exponentiate — the computation `hdr_map` collapses
/// algebraically. Every input here is above the log floor.
fn reference_hdr(px: &[f32], size: Size2us, scales: usize, amount: f32) -> Vec<f32> {
    let mut c_curr = Buffer2::new(size.width, size.height, px.iter().map(|v| v.ln()).collect());
    let mut c_next = Buffer2::new_default(size.width, size.height);
    let mut tmp = Buffer2::new_default(size.width, size.height);
    let mut layers: Vec<Vec<f32>> = Vec::new();
    for j in 0..scales {
        atrous_smooth(&c_curr, &mut c_next, &mut tmp, 1 << j);
        layers.push(
            c_curr
                .pixels()
                .iter()
                .zip(c_next.pixels())
                .map(|(&c, &n)| c - n)
                .collect(),
        );
        mem::swap(&mut c_curr, &mut c_next);
    }
    let residual = c_curr.pixels();
    let mean = (residual.iter().map(|&v| f64::from(v)).sum::<f64>() / residual.len() as f64) as f32;
    let keep = 1.0 - amount;
    (0..size.pixel_count())
        .map(|i| {
            let flattened = mean + keep * (residual[i] - mean);
            let details: f32 = layers.iter().map(|l| l[i]).sum();
            (flattened + details).exp().clamp(0.0, 1.0)
        })
        .collect()
}

#[test]
fn hdr_matches_explicit_pyramid_reference() {
    let size = Size2us::new(64, 48);
    let (scales, amount) = (3, 0.6);
    let px = dome(size);
    let mut img = gray(size, px.clone());
    Hdr { scales, amount }.apply(&mut img).unwrap();
    let out = img.channel(0);
    let expected = reference_hdr(&px, size, scales, amount);
    // The log plane lies in [ln 0.1, 0], under 2.31 in size. The reference rounds each of its three
    // layer differences, their two sums, the residual's flattening twice and the final sum, each
    // by up to an ulp of 2.31 (2ε of it); the collapse rounds a handful of times on its own. Each
    // absolute error δ of the log is a relative δ of the output, which is at most 1, and `exp`
    // and the clamp add 2ε: under (10·2·2.31 + 2)ε ≈ 48ε absolute.
    assert_close_slice!(
        out.pixels(),
        expected,
        48.0 * f32::EPSILON,
        "collapsed vs pyramid"
    );
}

/// A flat plane has no large-scale contrast to compress: whatever the amount, it comes back as
/// itself. Its log residual is `ln 0.2` to the smoothing's rounding, so its mean is too — when the
/// mean is taken in f64. A sequential f32 fold over these 262 144 samples drifts by about n·ε/2
/// = 1.6% of ln 0.2, which `amount` = 0.9 would carry into every pixel; the smoothing's own
/// rounding is a few ulps of |ln 0.2| = 1.61, two of which move the factor by 2·2ε·1.61, held here
/// with the product's rounding to 8ε relative.
#[test]
fn a_flat_plane_comes_back_as_itself() {
    let size = Size2us::new(512, 512);
    let mut img = gray(size, vec![0.2; size.pixel_count()]);
    Hdr {
        scales: 5,
        amount: 0.9,
    }
    .apply(&mut img)
    .unwrap();
    let bound = 8.0 * f32::EPSILON * 0.2;
    for &v in img.channel(0).pixels() {
        assert!((v - 0.2).abs() <= bound, "{v}");
    }
}

/// On colour the compression stays in display range on its own — the remap divides by a channel
/// past white rather than clipping it — on a dome whose red runs twice its intensity.
#[test]
fn hdr_keeps_colour_in_range() {
    let size = Size2us::new(96, 96);
    let i = dome(size);
    let r: Vec<f32> = i.iter().map(|&v| (2.0 * v).min(1.0)).collect();
    let mut img = rgb(size, r, i.clone(), i);
    Hdr::default().apply(&mut img).unwrap();
    for channel in 0..3 {
        for &v in img.channel(channel).pixels() {
            assert!((0.0..=1.0).contains(&v), "channel {channel}: {v}");
        }
    }
}

/// More scales move more structure into the compressed residual, so 2 and 4 scales differ; past
/// what the frame holds (`max_scales`, 5 for 48 px) a request is that limit, bit for bit.
#[test]
fn scales_change_the_output_up_to_the_frame_limit() {
    let size = Size2us::new(48, 48);
    let run = |scales| {
        let mut img = gray(size, dome(size));
        Hdr {
            scales,
            amount: 0.6,
        }
        .apply(&mut img)
        .unwrap();
        img.channel(0).pixels().to_vec()
    };
    assert_eq!(max_scales(size), 5);
    assert_ne!(run(2), run(4));
    assert_eq!(run(20), run(5));
}

/// A faint halo beside a bright core keeps its light. The core, a disk of radius 6 at 1.0 on a sky
/// of 0.02, fills the residual around it, so a halo pixel 8 px from its centre sits far below its
/// local base. Subtracting the compressed base takes such a pixel below black — the control shows
/// it — while the log domain scales it by a positive factor: every pixel comes out above 0.
#[test]
fn a_halo_pixel_stays_above_black() {
    let size = Size2us::new(64, 64);
    let centre = Vec2us::new(32, 32);
    let px: Vec<f32> = (0..size.pixel_count())
        .map(|index| {
            let p = size.point_of(index);
            let (dx, dy) = (p.x as f32 - centre.x as f32, p.y as f32 - centre.y as f32);
            if dx * dx + dy * dy <= 36.0 { 1.0 } else { 0.02 }
        })
        .collect();
    let (scales, amount) = (4, 0.8);
    let halo = size.index_of(Vec2us::new(40, 32));
    assert!(
        linear_hdr(&px, size, scales, amount)[halo] < 0.0,
        "the control: subtraction takes the halo below black"
    );
    let mut img = gray(size, px);
    Hdr { scales, amount }.apply(&mut img).unwrap();
    let out = img.channel(0).pixels();
    assert!(
        out.iter().all(|&v| v > 0.0),
        "min {}",
        out.iter().fold(1.0f32, |a, &b| a.min(b))
    );
}

/// A near-black pixel keeps its ratio to its neighbour: both take the factor of the base around
/// them, so 1e-3 beside 1e-4 in a dark corner stays near ten times brighter. Their bases differ
/// only through their own logs, which the composite kernel weighs near its centre; the output
/// ratio is `10·exp(−amount·Δ)` for the difference `Δ` of their log residuals, to the roundings
/// of the two factors and the quotient (8ε), and here 0.8% from 10. Subtraction instead lifts both
/// by the same amount, which turns the noise of a near-black region into bright speckle: the
/// control's ratio is near 1.
#[test]
fn near_black_pixels_keep_their_ratio() {
    let size = Size2us::new(64, 64);
    let mut px = dome(size);
    let (dim, bright) = (
        size.index_of(Vec2us::new(2, 2)),
        size.index_of(Vec2us::new(3, 2)),
    );
    px[dim] = 1e-4;
    px[bright] = 1e-3;
    let (scales, amount) = (4, 0.8);
    let control = linear_hdr(&px, size, scales, amount);
    assert!(
        control[bright] / control[dim] < 1.5,
        "the control: subtraction lifts both alike"
    );
    let logs: Vec<f32> = px.iter().map(|v| v.ln()).collect();
    let base = residual(&logs, size, scales);
    let expected = (px[bright] / px[dim]) * (-amount * (base[bright] - base[dim])).exp();
    let mut img = rgb(size, px.clone(), px.clone(), px);
    Hdr { scales, amount }.apply(&mut img).unwrap();
    let out = img.channel(0).pixels();
    let ratio = out[bright] / out[dim];
    assert!(
        (ratio - expected).abs() <= 8.0 * f32::EPSILON * expected,
        "{ratio} vs {expected}"
    );
    assert!((ratio - 10.0).abs() <= 0.1, "{ratio}");
}

/// A pixel with no positive intensity is black on grey and on colour alike.
#[test]
fn no_positive_intensity_is_black_on_grey_and_colour() {
    let size = Size2us::new(16, 16);
    let mut px = vec![0.3f32; size.pixel_count()];
    px[0] = -0.01;
    px[1] = 0.0;
    let mut grey = gray(size, px.clone());
    let mut colour = rgb(size, px.clone(), px.clone(), px);
    let hdr = Hdr {
        scales: 2,
        amount: 0.5,
    };
    hdr.apply(&mut grey).unwrap();
    hdr.apply(&mut colour).unwrap();
    for index in [0, 1] {
        assert_eq!(grey.channel(0).pixels()[index], 0.0);
        for channel in 0..3 {
            assert_eq!(colour.channel(channel).pixels()[index], 0.0);
        }
    }
}
