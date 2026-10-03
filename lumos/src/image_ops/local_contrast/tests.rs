use crate::image_ops::error::OpError;
use crate::image_ops::local_contrast::{
    LocalContrast, N_BINS, TileAxis, build_tile_luts, clip_histogram,
};
use crate::testing::images::{gray_image as gray, rgb_image as rgb};
use crate::testing::prelude::*;
use crate::testing::synthetic::metrics::pixel_stats;

/// A low-contrast horizontal gradient (intensity in `[0.45, 0.55]`).
fn low_contrast(size: Size2us) -> Vec<f32> {
    (0..size.pixel_count())
        .map(|i| 0.45 + 0.1 * (size.point_of(i).x as f32 / (size.width - 1) as f32))
        .collect()
}

#[test]
fn clahe_strength_zero_is_identity() {
    let px = low_contrast(Size2us::new(64, 64));
    let mut img = gray(Size2us::new(64, 64), px.clone());
    LocalContrast {
        strength: 0.0,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    assert_eq!(
        img.channel(0).to_vec(),
        px,
        "strength 0 leaves the image untouched"
    );
}

#[test]
fn clahe_output_stays_in_range() {
    let px: Vec<f32> = (0..96 * 96)
        .map(|i| ((i as f32 * 0.013).sin() * 0.5 + 0.5).clamp(0.0, 1.0))
        .collect();
    let mut img = gray(Size2us::new(96, 96), px);
    LocalContrast::default().apply(&mut img).unwrap();
    for &v in &img.channel(0).to_vec() {
        assert!((0.0..=1.0).contains(&v), "output in [0,1]: {v}");
    }
}

#[test]
fn clahe_flat_region_not_blown_up() {
    // Contrast-limited: a flat field must stay put, not get stretched to full range.
    let mut img = gray(Size2us::new(64, 64), vec![0.5; 64 * 64]);
    LocalContrast::default().apply(&mut img).unwrap();
    let out = img.channel(0).to_vec();
    assert!(
        out.iter().all(|&v| (v - 0.5).abs() < 0.05),
        "flat 0.5 stays ~0.5 (mean {})",
        (pixel_stats(&out).mean as f32)
    );
}

#[test]
fn clahe_increases_low_contrast() {
    // A low-contrast gradient gets its local contrast expanded → higher spread.
    let px = low_contrast(Size2us::new(64, 64));
    let in_std = pixel_stats(&px).std as f32;
    let mut img = gray(Size2us::new(64, 64), px);
    LocalContrast {
        tiles: 4,
        clip_limit: 4.0,
        strength: 1.0,
    }
    .apply(&mut img)
    .unwrap();
    let out_std = pixel_stats(img.channel(0)).std as f32;
    assert!(
        out_std > in_std,
        "local contrast expanded: {out_std} > {in_std}"
    );
}

#[test]
fn clahe_tile_mappings_are_monotonic() {
    let px: Vec<f32> = (0..80 * 80)
        .map(|i| f32::midpoint((i % 80) as f32 / 79.0, (i / 80) as f32 / 79.0))
        .collect();
    let intensity = Buffer2::new(80, 80, px);
    let axis = TileAxis::new(80, 4);
    let luts = build_tile_luts(&intensity, &axis, &axis, 2.0);
    for lut in &luts {
        for w in lut.windows(2) {
            assert!(
                w[1] >= w[0] - 1e-6,
                "LUT must be non-decreasing: {} -> {}",
                w[0],
                w[1]
            );
        }
    }
}

#[test]
fn clahe_is_color_preserving() {
    // A 2:1:1 R:G:B field keeps its ratio (hue) through the intensity-based mapping.
    let size = Size2us::new(64, 64);
    let i: Vec<f32> = low_contrast(size); // use as the green/blue level
    let r: Vec<f32> = i.iter().map(|&v| (2.0 * v).min(1.0)).collect();
    let mut img = rgb(size, r, i.clone(), i.clone());
    LocalContrast::default().apply(&mut img).unwrap();
    let (ro, go, bo) = (
        img.channel(0).to_vec(),
        img.channel(1).to_vec(),
        img.channel(2).to_vec(),
    );
    assert_eq!(go, bo, "G and B stay equal");
    // Where red isn't clamped at 1, the 2:1 ratio is preserved.
    for k in 0..ro.len() {
        if ro[k] < 0.999 && go[k] > 1e-3 {
            assert!(
                (ro[k] / go[k] - 2.0).abs() < 0.05,
                "R:G ratio preserved at {k}: {} {}",
                ro[k],
                go[k]
            );
        }
    }
}

#[test]
fn rejects_clip_limit_below_one() {
    let mut img = gray(Size2us::new(8, 8), vec![0.5; 64]);
    let err = LocalContrast {
        clip_limit: 0.5,
        strength: 0.0,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap_err();
    assert!(
        matches!(&err, OpError::InvalidConfig(m) if m.field == "local contrast clip_limit"),
        "expected an InvalidConfig clip_limit error, got {err:?}"
    );
}

/// Clipping hands the excess back evenly. 1000 in bin 0 clipped at 10 frees 990: 3 to every bin
/// (768), and the remaining 222 one apiece at a stride of ⌊256/222⌋ = 1, so bins 0..222 — bin 0 ends
/// at 10 + 3 + 1. A remainder of 100 strides by 2: bins 0, 2, …, 198. The count is kept.
#[test]
fn clipping_spreads_the_excess_across_the_range() {
    let mut hist = [0u32; N_BINS];
    hist[0] = 1000;
    clip_histogram(&mut hist, 10);
    let mut expected = [3u32; N_BINS];
    for bin in &mut expected[..222] {
        *bin += 1;
    }
    expected[0] += 10;
    assert_eq!(hist, expected);
    assert_eq!(hist.iter().sum::<u32>(), 1000);

    let mut hist = [0u32; N_BINS];
    hist[100] = 10 + 100;
    clip_histogram(&mut hist, 10);
    let mut expected = [0u32; N_BINS];
    expected[100] = 10;
    for bin in (0..200).step_by(2) {
        expected[bin] += 1;
    }
    assert_eq!(hist, expected);
}

/// A tile whose histogram is already flat maps every value to itself: 4096 values `(i + ½)/4096`
/// put 16 in each bin, under the clip of 2 · 16, so each bin edge sits at its own level and the
/// mapping between edges is the identity, to the rounding of the interpolation (4ε).
#[test]
fn a_flat_histogram_maps_to_the_identity() {
    let size = Size2us::new(64, 64);
    let px: Vec<f32> = (0..4096).map(|i| (i as f32 + 0.5) / 4096.0).collect();
    let mut img = gray(size, px.clone());
    LocalContrast {
        tiles: 1,
        clip_limit: 2.0,
        strength: 1.0,
    }
    .apply(&mut img)
    .unwrap();
    for (&out, &v) in img.channel(0).pixels().iter().zip(&px) {
        assert!((out - v).abs() <= 4.0 * f32::EPSILON, "{v} → {out}");
    }
}

/// The mapping is continuous, not one level per bin: 4096 distinct values of a skewed ramp come
/// out strictly increasing, where a 256-entry lookup gives each run of 16 one output.
#[test]
fn distinct_values_stay_distinct() {
    let size = Size2us::new(64, 64);
    let px: Vec<f32> = (0..4096).map(|i| (i as f32 / 4095.0).powi(2)).collect();
    let mut img = gray(size, px);
    LocalContrast {
        tiles: 1,
        clip_limit: 3.0,
        strength: 1.0,
    }
    .apply(&mut img)
    .unwrap();
    for pair in img.channel(0).pixels().windows(2) {
        assert!(pair[1] > pair[0], "{} then {}", pair[0], pair[1]);
    }
}

/// Tiles split an axis evenly and are never empty: 9 px in 4 tiles have edges 0, 2, 4, 6, 9 and
/// centres 0.5, 2.5, 4.5, 7. A pixel blends the two tiles whose centres bracket it — x = 1 lies a
/// quarter of the way from 0.5 to 2.5 — and past the outer centres takes the edge tile alone.
#[test]
fn tiles_split_evenly_and_blend_between_centres() {
    let axis = TileAxis::new(9, 4);
    assert_eq!(axis.bounds, [0, 2, 4, 6, 9]);
    assert_eq!(axis.centres, [0.5, 2.5, 4.5, 7.0]);
    for (x, lower, upper, weight) in [
        (0, 0, 0, 0.0),
        (1, 0, 1, 0.25),
        (5, 2, 3, 0.2),
        (7, 3, 3, 0.0),
        (8, 3, 3, 0.0),
    ] {
        let blend = axis.blend(x);
        assert_eq!(
            (blend.lower, blend.upper, blend.weight),
            (lower, upper, weight),
            "x = {x}"
        );
    }
    assert_eq!(
        TileAxis::new(3, 8).bounds,
        [0, 1, 2, 3],
        "at most one tile per pixel"
    );
}
