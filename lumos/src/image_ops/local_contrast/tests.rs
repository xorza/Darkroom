use crate::image_ops::local_contrast::{
    LocalContrast, N_BINS, TileAxis, build_tile_luts, clip_histogram,
};
use crate::testing::images::{gray_image as gray, rgb_image as rgb};
use crate::testing::prelude::*;
use crate::testing::synthetic::metrics::pixel_stats;
use crate::testing::synthetic::patterns;

/// A low-contrast horizontal gradient, intensity 0.45 to 0.55.
fn low_contrast(size: Size2us) -> Vec<f32> {
    patterns::horizontal_gradient(size, 0.45, 0.55).into_vec()
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

/// On colour the enhancement stays in display range on its own — the remap divides by a channel
/// past white rather than clipping it — on a field whose red runs twice its intensity.
#[test]
fn clahe_keeps_colour_in_range() {
    let size = Size2us::new(96, 96);
    let i: Vec<f32> = (0..size.pixel_count())
        .map(|i| ((i as f32 * 0.013).sin() * 0.5 + 0.5).clamp(0.0, 1.0))
        .collect();
    let r: Vec<f32> = i.iter().map(|&v| (2.0 * v).min(1.0)).collect();
    let mut img = rgb(size, r, i.clone(), i);
    LocalContrast::default().apply(&mut img).unwrap();
    for channel in 0..3 {
        for &v in img.channel(channel).pixels() {
            assert!((0.0..=1.0).contains(&v), "channel {channel}: {v}");
        }
    }
}

/// A flat field is held exactly. 64 × 64 caps the grid at 2 × 2 tiles of 1024 pixels, all in bin
/// 128: clipped at 2 · 1024/256 = 8, the 1016 freed give every bin 3 and bins 0..248 one more, so
/// 128 bins below put the bin's lower edge at 128 · 4 / 1024 = 0.5 — where 0.5 maps, exactly.
#[test]
fn clahe_flat_region_not_blown_up() {
    let mut img = gray(Size2us::new(64, 64), vec![0.5; 64 * 64]);
    LocalContrast::default().apply(&mut img).unwrap();
    assert!(img.channel(0).pixels().iter().all(|&v| v == 0.5));
}

/// A low-contrast gradient comes out with a wider spread, and both knobs reach the result: the
/// clip limit changes how far each tile stretches, the tile count where the tiles fall. 128 × 128
/// holds `√(16384 / (4 · 256))` = 4 tiles a side before the cap, so 4 and 2 both stand.
#[test]
fn clahe_increases_low_contrast_and_every_knob_matters() {
    let size = Size2us::new(128, 128);
    let px = low_contrast(size);
    let in_std = pixel_stats(&px).std;
    let run = |tiles, clip_limit| {
        let mut img = gray(size, px.clone());
        LocalContrast {
            tiles,
            clip_limit,
            strength: 1.0,
        }
        .apply(&mut img)
        .unwrap();
        img.channel(0).to_vec()
    };
    let base = run(4, 4.0);
    let clip = run(4, 2.0);
    let tiles = run(2, 4.0);
    for (name, out) in [("base", &base), ("clip", &clip), ("tiles", &tiles)] {
        let out_std = pixel_stats(out).std;
        assert!(out_std > in_std, "{name}: {out_std} > {in_std}");
    }
    assert_ne!(base, clip, "clip_limit matters");
    assert_ne!(base, tiles, "tiles matters");
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
                w[1] >= w[0],
                "LUT must be non-decreasing: {} -> {}",
                w[0],
                w[1]
            );
        }
    }
}

/// A 2:1:1 field keeps its hue exactly: every channel of a pixel is scaled by one factor, and
/// doubling commutes with rounding, so wherever the input red is twice the green the output is too.
#[test]
fn clahe_is_color_preserving() {
    let size = Size2us::new(64, 64);
    let i = low_contrast(size);
    let r: Vec<f32> = i.iter().map(|&v| (2.0 * v).min(1.0)).collect();
    let mut img = rgb(size, r.clone(), i.clone(), i.clone());
    LocalContrast::default().apply(&mut img).unwrap();
    let (ro, go, bo) = (img.channel(0), img.channel(1), img.channel(2));
    assert_eq!(go.pixels(), bo.pixels(), "G and B stay equal");
    let mut checked = 0;
    for k in 0..ro.len() {
        if r[k] < 1.0 {
            assert_eq!(ro[k], 2.0 * go[k], "R:G at {k}");
            checked += 1;
        }
    }
    assert!(checked > 0);
}

/// Clipping hands the excess back evenly. 1000 in bin 0 clipped at 10 frees 990: 3 to every bin
/// (768), and the remaining 222 one apiece at a stride of ⌊256/222⌋ = 1, so bins 0..222 — bin 0
/// ends at 10 + 3 + 1. A remainder of 100 strides by 2: bins 0, 2, …, 198. The count is kept.
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
