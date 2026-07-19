use crate::image_ops::wavelet::{atrous_smooth, max_scales, reflect};
use imaginarium::Buffer2;

fn pattern(width: usize, height: usize) -> Buffer2<f32> {
    let px = (0..width * height)
        .map(|i| {
            let (x, y) = ((i % width) as f32, (i / width) as f32);
            0.5 + 0.3 * (x * 0.3).sin() * (y * 0.2).cos()
        })
        .collect();
    Buffer2::new(width, height, px)
}

#[test]
fn reflect_mirrors_indices() {
    let n = 5; // period = 2*(5-1) = 8
    assert_eq!(reflect(0, n), 0);
    assert_eq!(reflect(4, n), 4);
    assert_eq!(reflect(-1, n), 1);
    assert_eq!(reflect(-2, n), 2);
    assert_eq!(reflect(-3, n), 3);
    assert_eq!(reflect(5, n), 3);
    assert_eq!(reflect(6, n), 2);
    assert_eq!(reflect(7, n), 1);
    assert_eq!(reflect(8, n), 0);
    assert_eq!(reflect(10, n), 2);
    assert_eq!(reflect(-10, n), 2);
    assert_eq!(reflect(3, 1), 0);
    assert_eq!(reflect(-3, 1), 0);
}

#[test]
fn max_scales_bounds_by_dimension() {
    assert_eq!(max_scales(1, 1), 1);
    assert_eq!(max_scales(2, 2), 1);
    assert_eq!(max_scales(5, 5), 2);
    assert_eq!(max_scales(8, 8), 3);
    assert_eq!(max_scales(1000, 8), 3);
}

#[test]
fn atrous_smooth_preserves_constant() {
    // The B3 kernel sums to 1, so a flat field is reproduced exactly at every hole spacing.
    let (w, h) = (8, 6);
    let src = Buffer2::new(w, h, vec![0.42; w * h]);
    let mut dst = Buffer2::new_default(w, h);
    let mut tmp = Buffer2::new_default(w, h);
    for step in [1usize, 2, 4] {
        atrous_smooth(&src, &mut dst, &mut tmp, step);
        for &v in dst.pixels() {
            assert!(
                (v - 0.42).abs() < 1e-6,
                "constant preserved at step {step}: {v}"
            );
        }
    }
}

#[test]
fn starlet_details_telescope_exactly() {
    // The starlet identity `image == c_J + Σ (c_j − c_{j+1})`, within f32 rounding.
    // Both consumers (denoise, hdr) stream the transform and lean on this identity
    // instead of materializing layers — pin it against `atrous_smooth` directly.
    let img = pattern(17, 13);
    let mut c_curr = img.clone();
    let mut c_next = Buffer2::new_default(17, 13);
    let mut tmp = Buffer2::new_default(17, 13);
    let mut details_sum = vec![0.0f32; 17 * 13];
    for j in 0..4 {
        atrous_smooth(&c_curr, &mut c_next, &mut tmp, 1 << j);
        for ((s, &c), &n) in details_sum
            .iter_mut()
            .zip(c_curr.pixels())
            .zip(c_next.pixels())
        {
            *s += c - n;
        }
        std::mem::swap(&mut c_curr, &mut c_next);
    }
    for ((&orig, &residual), &details) in img.pixels().iter().zip(c_curr.pixels()).zip(&details_sum)
    {
        let recon = residual + details;
        assert!(
            (recon - orig).abs() < 1e-4,
            "residual + Σ details reconstructs: {recon} vs {orig}"
        );
    }
}
