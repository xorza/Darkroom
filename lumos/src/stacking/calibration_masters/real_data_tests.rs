//! Real-data calibration-master benchmarks + a build smoke test over the bundled
//! `test_data/lumos_data/{Bias,Darks,Flats}` Fuji X-Trans RAF set.
//!
//! These drive the **canonical** master-build API — `CalibrationMasters::from_files`
//! → `stack_cfa_master` → `CfaCache`/`run_stacking` + defect-map derivation — the same
//! path `lens` calls, including the libraw RAW decode of every calibration frame. That
//! decode dominates the wall time, so these measure the real end-to-end cost of producing
//! masters, not the isolated combine kernel.
//!
//! Gated behind the `real-data` feature (the dataset is gitignored; fetch it with
//! `scripts/fetch-test-data.sh`). The `#[quick_bench]` fns are also `#[ignore]`.
//!
//! Run:
//!   cargo test -p lumos --release --features real-data \
//!     calibration_masters::real_data_tests -- --ignored --nocapture

use std::hint::black_box;
use std::path::PathBuf;

use common::CancelToken;
use common::file_utils;
use quickbench::quick_bench;

use crate::io::raw;
use crate::stacking::calibration_masters::defect_map::DefectMap;
use crate::stacking::calibration_masters::stack_cfa_master;
use crate::testing::{calibration_dir, init_tracing};
use crate::{CalibrationMasters, CalibrationSet, CfaImage, DEFAULT_SIGMA_THRESHOLD, StackConfig};

/// Bundled calibration frame paths grouped by role (no flat-darks in this set).
#[derive(Debug)]
struct CalibrationPaths {
    darks: Vec<PathBuf>,
    flats: Vec<PathBuf>,
    bias: Vec<PathBuf>,
}

/// Collect the RAF frame paths for each calibration role, or `None` if the dataset
/// directory is absent (so a bench/test without the `real-data` bundle skips cleanly).
fn calibration_paths() -> Option<CalibrationPaths> {
    let dir = calibration_dir();
    let raw = |sub: &str| {
        file_utils::files_with_extensions(&dir.join(sub), raw::RAW_EXTENSIONS)
            .expect("scan RAW calibration directory")
    };
    let paths = CalibrationPaths {
        darks: raw("Darks"),
        flats: raw("Flats"),
        bias: raw("Bias"),
    };
    if paths.darks.is_empty() || paths.flats.is_empty() || paths.bias.is_empty() {
        return None;
    }
    Some(paths)
}

#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn raw_dimensions_matches_full_decode() {
    // `from_files` sizes its in-memory-vs-disk decision from `raw_dimensions` (a header peek, no
    // decode). That peek must report exactly the dims a full decode produces, or the memory budget
    // would be wrong.
    let Some(paths) = calibration_paths() else {
        panic!("calibration frames missing — run scripts/fetch-test-data.sh");
    };
    let path = &paths.darks[0];
    let peeked = raw::raw_dimensions(path).expect("peek dims");
    let loaded = raw::load_raw_cfa(path).expect("full decode");
    assert_eq!(
        (peeked.x, peeked.y),
        (loaded.data.width(), loaded.data.height()),
        "peeked header dims must match the decoded frame"
    );
}

#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn builds_full_master_set() {
    init_tracing();
    let Some(paths) = calibration_paths() else {
        panic!("calibration frames missing — run scripts/fetch-test-data.sh");
    };

    let masters = CalibrationMasters::from_files(
        CalibrationSet {
            dark: &paths.darks,
            flat: &paths.flats,
            bias: &paths.bias,
            flat_dark: &[],
        },
        DEFAULT_SIGMA_THRESHOLD,
    )
    .expect("master build failed");

    // Every supplied role yields a master; the un-supplied flat-dark stays `None`.
    let dark = masters.dark.as_ref().expect("master dark");
    let flat = masters.flat.as_ref().expect("prepared master flat");
    let bias = masters.bias.as_ref().expect("master bias");
    assert!(masters.flat_dark.is_none());

    // All masters share the single sensor geometry (one CFA plane each).
    let (w, h) = (dark.data.width(), dark.data.height());
    assert!(w > 0 && h > 0, "degenerate master dimensions {w}x{h}");
    assert_eq!(
        (flat.data.width(), flat.data.height()),
        (w, h),
        "flat master dimensions differ from dark"
    );
    assert_eq!(
        (bias.data.width(), bias.data.height()),
        (w, h),
        "bias master dimensions differ from dark"
    );

    // A defect map is derived whenever a dark or flat is present (hot from the dark,
    // cold from the flat), so the full set must produce one.
    assert!(
        masters.defect_map.is_some(),
        "defect map should be derived from dark + flat"
    );
    println!("  defects: {:?}", masters.defect_summary().unwrap());

    // Masters are calibration-normalized CFA data: `(value - black) * inv_range`, deliberately
    // *unclamped* (unlike the light path) so master dark/bias keep their signed noise
    // distribution — black-level subtraction centers an unilluminated frame near 0, so a few
    // pixels dip just below it. Hard invariants: every pixel finite (the combine reducers
    // assume it), each master non-degenerate (real variation, not a flat buffer), and all
    // values within a sane normalized envelope.
    let mean_of = |m: &CfaImage, name: &str| -> f32 {
        let px = m.data.pixels();
        assert!(
            px.iter().all(|v| v.is_finite()),
            "{name} master has non-finite pixels"
        );
        let (min, max) = px
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let mean = px.iter().sum::<f32>() / px.len() as f32;
        println!("  master {name}: min {min:.4}, mean {mean:.4}, max {max:.4}");
        assert!(max > min, "{name} master is degenerate (constant buffer)");
        assert!(
            (-0.5..=2.0).contains(&min) && (-0.5..=2.0).contains(&max),
            "{name} master values outside sane envelope [{min}, {max}]"
        );
        mean
    };
    mean_of(dark, "dark");
    mean_of(bias, "bias");

    let flat_pixels = flat.data.pixels();
    assert!(flat_pixels.iter().all(|value| value.is_finite()));
    assert!(flat_pixels.iter().all(|&value| value >= 0.1));
    let (flat_min, flat_max) = flat_pixels
        .iter()
        .fold((f32::MAX, f32::MIN), |(min, max), &value| {
            (min.min(value), max.max(value))
        });
    assert!(
        flat_max > flat_min,
        "prepared flat is degenerate (constant buffer)"
    );
}

#[derive(Debug)]
struct HotMaskMetrics {
    hot_count: usize,
    edge_count: usize,
    max_bin_count: usize,
}

#[derive(Debug)]
struct DetectedHotMask {
    map: DefectMap,
    width: usize,
    height: usize,
}

fn hot_mask_metrics(map: &DefectMap, width: usize, height: usize) -> HotMaskMetrics {
    const BINS: usize = 8;

    let margin_x = width / 10;
    let margin_y = height / 10;
    let edge_count = map
        .hot_indices
        .iter()
        .filter(|&&index| {
            let x = index % width;
            let y = index / width;
            x < margin_x || x >= width - margin_x || y < margin_y || y >= height - margin_y
        })
        .count();
    let mut bins = [0usize; BINS * BINS];
    for &index in &map.hot_indices {
        let x = index % width;
        let y = index / width;
        let bx = (x * BINS / width).min(BINS - 1);
        let by = (y * BINS / height).min(BINS - 1);
        bins[by * BINS + bx] += 1;
    }

    HotMaskMetrics {
        hot_count: map.hot_indices.len(),
        edge_count,
        max_bin_count: *bins.iter().max().unwrap(),
    }
}

fn sorted_intersection_count(left: &[usize], right: &[usize]) -> usize {
    let mut left_index = 0;
    let mut right_index = 0;
    let mut count = 0;
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Equal => {
                count += 1;
                left_index += 1;
                right_index += 1;
            }
            std::cmp::Ordering::Greater => right_index += 1,
        }
    }
    count
}

#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn hot_mask_spatial_distribution_and_repeatability() {
    let Some(paths) = calibration_paths() else {
        panic!("calibration frames missing — run scripts/fetch-test-data.sh");
    };
    let first_paths: Vec<_> = paths
        .darks
        .iter()
        .enumerate()
        .filter_map(|(index, path)| index.is_multiple_of(2).then_some(path))
        .collect();
    let second_paths: Vec<_> = paths
        .darks
        .iter()
        .enumerate()
        .filter_map(|(index, path)| (!index.is_multiple_of(2)).then_some(path))
        .collect();
    let full_paths: Vec<_> = paths.darks.iter().collect();

    let detect = |dark_paths: &[&PathBuf]| {
        let dark = stack_cfa_master(dark_paths, StackConfig::dark(), CancelToken::never())
            .expect("master-dark stack failed")
            .expect("master dark");
        let width = dark.data.width();
        let height = dark.data.height();
        let map = DefectMap::default()
            .detect_hot(&dark, DEFAULT_SIGMA_THRESHOLD, &CancelToken::never())
            .expect("hot detection failed");
        DetectedHotMask { map, width, height }
    };

    let first = detect(&first_paths);
    let second = detect(&second_paths);
    let full = detect(&full_paths);
    assert_eq!((second.width, second.height), (first.width, first.height));
    assert_eq!((full.width, full.height), (first.width, first.height));

    let intersection = sorted_intersection_count(&first.map.hot_indices, &second.map.hot_indices);
    let union = first.map.hot_indices.len() + second.map.hot_indices.len() - intersection;
    let jaccard = intersection as f32 / union as f32;
    let first_metrics = hot_mask_metrics(&first.map, first.width, first.height);
    let second_metrics = hot_mask_metrics(&second.map, second.width, second.height);
    let full_metrics = hot_mask_metrics(&full.map, full.width, full.height);
    println!(
        "  alternating master-dark hot masks: {} and {}, intersection {}, Jaccard {:.4}",
        first.map.hot_indices.len(),
        second.map.hot_indices.len(),
        intersection,
        jaccard
    );
    println!("  first spatial metrics: {first_metrics:?}");
    println!("  second spatial metrics: {second_metrics:?}");
    println!("  full spatial metrics: {full_metrics:?}");

    assert_eq!(first_metrics.hot_count, 50_248);
    assert_eq!(first_metrics.edge_count, 18_598);
    assert_eq!(first_metrics.max_bin_count, 871);
    assert_eq!(second_metrics.hot_count, 50_341);
    assert_eq!(second_metrics.edge_count, 18_606);
    assert_eq!(second_metrics.max_bin_count, 866);
    assert_eq!(full_metrics.hot_count, 51_279);
    assert_eq!(full_metrics.edge_count, 18_984);
    assert_eq!(full_metrics.max_bin_count, 885);
    assert_eq!(intersection, 45_296);
}

#[quick_bench(warmup_iters = 0, iters = 1)]
fn bench_build_masters_from_files(b: ::quickbench::Bencher) {
    let Some(paths) = calibration_paths() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };
    println!(
        "Building full master set: {} darks + {} flats + {} bias",
        paths.darks.len(),
        paths.flats.len(),
        paths.bias.len(),
    );
    b.bench(|| {
        black_box(
            CalibrationMasters::from_files(
                CalibrationSet {
                    dark: &paths.darks,
                    flat: &paths.flats,
                    bias: &paths.bias,
                    flat_dark: &[],
                },
                DEFAULT_SIGMA_THRESHOLD,
            )
            .expect("master build failed"),
        )
    });
}

#[quick_bench(warmup_iters = 0, iters = 1)]
fn bench_stack_master_dark(b: ::quickbench::Bencher) {
    let Some(paths) = calibration_paths() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };
    println!("Stacking master dark from {} frames", paths.darks.len());
    b.bench(|| {
        black_box(
            stack_cfa_master(&paths.darks, StackConfig::dark(), CancelToken::never())
                .expect("dark stack failed"),
        )
    });
}

#[quick_bench(warmup_iters = 0, iters = 1)]
fn bench_stack_master_flat(b: ::quickbench::Bencher) {
    let Some(paths) = calibration_paths() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };
    println!("Stacking master flat from {} frames", paths.flats.len());
    b.bench(|| {
        black_box(
            stack_cfa_master(&paths.flats, StackConfig::flat(), CancelToken::never())
                .expect("flat stack failed"),
        )
    });
}

#[quick_bench(warmup_iters = 0, iters = 1)]
fn bench_stack_master_bias(b: ::quickbench::Bencher) {
    let Some(paths) = calibration_paths() else {
        eprintln!("No calibration data available, skipping benchmark");
        return;
    };
    println!("Stacking master bias from {} frames", paths.bias.len());
    b.bench(|| {
        black_box(
            stack_cfa_master(&paths.bias, StackConfig::bias(), CancelToken::never())
                .expect("bias stack failed"),
        )
    });
}
