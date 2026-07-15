use super::Hdr;
use crate::image_ops::op::OpError;
use crate::image_ops::test_support::channel_plane as channel;
use crate::image_ops::wavelet::atrous_smooth;
use imaginarium::{Buffer2, DeinterleavedImageData, Image};

fn gray(width: usize, height: usize, px: Vec<f32>) -> Image {
    Image::from(&DeinterleavedImageData::from_channels([Buffer2::new(
        width, height, px,
    )]))
}

/// Channel `c` of an image as a buffer (for assertions).
/// A smooth radial brightness dome — bright center (~1.0), dark corners (~0.1). The large-scale
/// brightness HDR is meant to compress.
fn dome(width: usize, height: usize) -> Vec<f32> {
    let (cx, cy) = ((width as f32 - 1.0) / 2.0, (height as f32 - 1.0) / 2.0);
    let sigma = width as f32 / 4.0;
    (0..width * height)
        .map(|i| {
            let (x, y) = ((i % width) as f32, (i / width) as f32);
            let r2 = (x - cx) * (x - cx) + (y - cy) * (y - cy);
            0.1 + 0.9 * (-r2 / (2.0 * sigma * sigma)).exp()
        })
        .collect()
}

#[test]
fn hdr_amount_zero_is_identity() {
    let px = dome(64, 64);
    let mut img = gray(64, 64, px.clone());
    Hdr {
        scales: 3,
        amount: 0.0,
    }
    .apply(&mut img)
    .unwrap();
    for (a, b) in channel(&img, 0).to_vec().iter().zip(&px) {
        assert!((a - b).abs() < 1e-4, "amount 0 is the identity: {a} vs {b}");
    }
}

#[test]
fn hdr_compresses_large_scale_contrast() {
    // The dome lives in the residual (scales=3 → residual captures >8 px structure); compressing it
    // shrinks the center-vs-corner contrast while keeping it monotone.
    let (w, h) = (128, 128);
    let px = dome(w, h);
    let ci = (h / 2) * w + w / 2;
    let in_contrast = px[ci] - px[0];
    let mut img = gray(w, h, px);
    Hdr {
        scales: 3,
        amount: 0.5,
    }
    .apply(&mut img)
    .unwrap();
    let out = channel(&img, 0).to_vec();
    let out_contrast = out[ci] - out[0];
    assert!(
        out_contrast < in_contrast * 0.7,
        "large-scale contrast compressed: {out_contrast} < {in_contrast}"
    );
    assert!(out_contrast > 0.0, "center stays brighter than the corner");
}

#[test]
fn hdr_amount_controls_compression() {
    let (w, h) = (128, 128);
    let px = dome(w, h);
    let ci = (h / 2) * w + w / 2;
    let contrast_at = |amount: f32| {
        let mut img = gray(w, h, px.clone());
        Hdr { scales: 3, amount }.apply(&mut img).unwrap();
        let o = channel(&img, 0).to_vec();
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
    let (w, h) = (128, 128);
    let mut px = dome(w, h);
    for (i, p) in px.iter_mut().enumerate() {
        *p += if (i % w + i / w) % 2 == 0 {
            0.03
        } else {
            -0.03
        };
    }
    let mut img = gray(w, h, px.clone());
    Hdr {
        scales: 3,
        amount: 0.6,
    }
    .apply(&mut img)
    .unwrap();
    let out = channel(&img, 0).to_vec();
    // Adjacent-pixel contrast along a dark row (corner side, no clipping) is the fine texture.
    let tex_in: f32 = (0..w - 1).map(|x| (px[x + 1] - px[x]).abs()).sum();
    let tex_out: f32 = (0..w - 1).map(|x| (out[x + 1] - out[x]).abs()).sum();
    assert!(
        tex_out > 0.5 * tex_in,
        "fine detail preserved: {tex_out} vs {tex_in}"
    );
}

/// The literal reference: materialize every detail layer, flatten the residual toward its
/// mean, re-sum — the computation `hdr_map` collapses algebraically.
fn reference_hdr(px: &[f32], w: usize, h: usize, scales: usize, amount: f32) -> Vec<f32> {
    let mut c_curr = Buffer2::new(w, h, px.to_vec());
    let mut c_next = Buffer2::new_default(w, h);
    let mut tmp = Buffer2::new_default(w, h);
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
        std::mem::swap(&mut c_curr, &mut c_next);
    }
    let residual = c_curr.pixels();
    let mean = residual.iter().sum::<f32>() / residual.len() as f32;
    let keep = 1.0 - amount;
    (0..w * h)
        .map(|i| {
            let flattened = mean + keep * (residual[i] - mean);
            let details: f32 = layers.iter().map(|l| l[i]).sum();
            (flattened + details).clamp(0.0, 1.0)
        })
        .collect()
}

#[test]
fn hdr_matches_explicit_pyramid_reference() {
    let (w, h) = (64, 48);
    let (scales, amount) = (3, 0.6);
    let px = dome(w, h);
    let mut img = gray(w, h, px.clone());
    Hdr { scales, amount }.apply(&mut img).unwrap();
    let out = channel(&img, 0);
    let expected = reference_hdr(&px, w, h, scales, amount);
    for (o, e) in out.pixels().iter().zip(&expected) {
        assert!(
            (o - e).abs() < 1e-5,
            "collapsed formula matches the layer pyramid: {o} vs {e}"
        );
    }
}

#[test]
fn hdr_output_stays_in_range() {
    let mut img = gray(96, 96, dome(96, 96));
    Hdr::default().apply(&mut img).unwrap();
    for &v in &channel(&img, 0).to_vec() {
        assert!((0.0..=1.0).contains(&v), "output in [0,1]: {v}");
    }
}

#[test]
fn rejects_out_of_range_amount() {
    let mut img = gray(8, 8, vec![0.5; 64]);
    let err = Hdr {
        scales: 6,
        amount: 1.5,
    }
    .apply(&mut img)
    .unwrap_err();
    assert!(
        matches!(&err, OpError::InvalidConfig(m) if m.contains("amount must be in")),
        "expected an InvalidConfig amount error, got {err:?}"
    );
}
