//! Real-data calibration-master checks over the bundled
//! `test_data/lumos_data/{Bias,Darks,Flats}` Fuji X-Trans RAF set.
//!
//! These drive the master-build path `lens` calls — `MasterStack::run` per role, then
//! `CalibrationMasters::from_images` and its defect-map derivation — including the libraw RAW
//! decode of every calibration frame.
//!
//! Gated behind the `real-data` feature (the dataset is gitignored; fetch it with
//! `scripts/fetch-test-data.sh`).

use std::cmp::Ordering;
use std::path::PathBuf;

use common::CancelToken;

use crate::calibration::calibration_masters::internals::masters_from_files;
use crate::calibration::defect_map::DefectMap;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::master_stack::MasterStack;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::{init_tracing, real_data};
use crate::io::load_context::LoadContext;
use crate::io::raw;
use crate::math::size2us::Size2us;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::{CalibrationSet, CfaImage, MasterRole, StackConfig};

#[test]
fn raw_frame_info_matches_full_decode() {
    // `from_files` sizes its in-memory-vs-disk decision from `raw::cfa_frame_info` (a header peek,
    // no decode). That peek must report exactly the dims a full decode produces, or the memory
    // budget would be wrong.
    let paths = real_data::calibration_frames();
    let path = &paths.darks[0];
    let peeked = raw::cfa_frame_info(path, &LoadContext::default()).expect("peek frame info");
    let loaded = raw::load_cfa(path, &LoadContext::default()).expect("full decode");
    assert_eq!(
        (peeked.dimensions.width(), peeked.dimensions.height()),
        (loaded.data().width(), loaded.data().height()),
        "peeked header dims must match the decoded frame"
    );
    assert_eq!(
        peeked.cfa_type,
        loaded.cfa_type(),
        "the peeked sensor pattern must be the decoded frame's"
    );
}

#[test]
fn builds_full_master_set() {
    init_tracing();
    let paths = real_data::calibration_frames();

    let masters = masters_from_files(
        CalibrationSet {
            dark: &paths.darks,
            flat: &paths.flats,
            bias: &paths.bias,
            flat_dark: &[],
        },
        SigmaThreshold::DEFAULT,
    );

    // Every supplied role yields a master; the flat-dark is spent on the flat and not kept.
    let dark = masters.dark.as_ref().expect("master dark");
    let flat = masters
        .flat
        .as_ref()
        .expect("prepared master flat")
        .divisor();
    let bias = masters.bias.as_ref().expect("master bias");

    // All masters share the single sensor geometry (one CFA plane each).
    let size = Size2us::from(dark.data());
    assert!(
        size.width > 0 && size.height > 0,
        "degenerate master dimensions {}x{}",
        size.width,
        size.height
    );
    assert_eq!(
        (flat.data().width(), flat.data().height()),
        (size.width, size.height),
        "flat master dimensions differ from dark"
    );
    assert_eq!(
        (bias.data().width(), bias.data().height()),
        (size.width, size.height),
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
    let check_master = |m: &CfaImage, name: &str| {
        let px = m.data().pixels();
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
    };
    check_master(dark, "dark");
    check_master(bias, "bias");

    let flat_pixels = flat.data().pixels();
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
    hot: usize,
    at_edge: usize,
    fullest_bin: usize,
}

#[derive(Debug)]
struct DetectedHotMask {
    map: DefectMap,
    size: Size2us,
}

fn hot_mask_metrics(map: &DefectMap, size: Size2us) -> HotMaskMetrics {
    const BINS: usize = 8;

    let margin_x = size.width / 10;
    let margin_y = size.height / 10;
    let at_edge = map
        .hot()
        .indices
        .iter()
        .filter(|&&index| {
            let p = size.point_of(index);
            let (x, y) = (p.x, p.y);
            x < margin_x
                || x >= size.width - margin_x
                || y < margin_y
                || y >= size.height - margin_y
        })
        .count();
    let mut bins = [0usize; BINS * BINS];
    for &index in &map.hot().indices {
        let p = size.point_of(index);
        let bx = (p.x * BINS / size.width).min(BINS - 1);
        let by = (p.y * BINS / size.height).min(BINS - 1);
        bins[by * BINS + bx] += 1;
    }

    HotMaskMetrics {
        hot: map.hot().indices.len(),
        at_edge,
        fullest_bin: *bins.iter().max().unwrap(),
    }
}

fn sorted_intersection_count(left: &[usize], right: &[usize]) -> usize {
    let mut left_index = 0;
    let mut right_index = 0;
    let mut count = 0;
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            Ordering::Less => left_index += 1,
            Ordering::Equal => {
                count += 1;
                left_index += 1;
                right_index += 1;
            }
            Ordering::Greater => right_index += 1,
        }
    }
    count
}

#[test]
fn hot_mask_spatial_distribution_and_repeatability() {
    let paths = real_data::calibration_frames();
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
        let dark = MasterStack {
            paths: dark_paths,
            role: MasterRole::Dark,
            config: StackConfig::bias_or_dark(),
            ingest: &IngestConfig::default(),
            subtract: &[],
        }
        .run(ProgressMonitor::unattended())
        .expect("master-dark stack failed")
        .expect("master dark");
        let size = Size2us::from(dark.data());
        let map = DefectMap::new(dark.size())
            .detect_hot(&dark, SigmaThreshold::DEFAULT, &CancelToken::never())
            .expect("hot detection failed");
        DetectedHotMask { map, size }
    };

    let first = detect(&first_paths);
    let second = detect(&second_paths);
    let full = detect(&full_paths);
    assert_eq!(second.size, first.size);
    assert_eq!(full.size, first.size);

    let intersection =
        sorted_intersection_count(&first.map.hot().indices, &second.map.hot().indices);
    let union = first.map.hot().indices.len() + second.map.hot().indices.len() - intersection;
    let jaccard = intersection as f32 / union as f32;
    let first_metrics = hot_mask_metrics(&first.map, first.size);
    let second_metrics = hot_mask_metrics(&second.map, second.size);
    let full_metrics = hot_mask_metrics(&full.map, full.size);
    println!(
        "  alternating master-dark hot masks: {} and {}, intersection {}, Jaccard {:.4}",
        first.map.hot().indices.len(),
        second.map.hot().indices.len(),
        intersection,
        jaccard
    );
    println!("  first spatial metrics: {first_metrics:?}");
    println!("  second spatial metrics: {second_metrics:?}");
    println!("  full spatial metrics: {full_metrics:?}");

    // Hot pixels are a property of the sensor, so the masks of two disjoint halves of the darks
    // agree: four in five of the pixels either flags, the other flags too.
    assert!(jaccard >= 0.8, "halves disagree: Jaccard {jaccard}");
    // And they sit across the frame as the sensor's defects do, uniformly: the band within a
    // tenth of each edge, 36% of the area, holds its share of them to within 5 points — amp glow
    // or an edge gradient leaking into the mask would load the band — and no 1/64 cell holds
    // more than 1.25 times the mean, which a gradient or a cluster of false positives would.
    let band_share = 1.0 - 0.8 * 0.8;
    for (name, metrics) in [
        ("first", &first_metrics),
        ("second", &second_metrics),
        ("full", &full_metrics),
    ] {
        let edge_share = metrics.at_edge as f64 / metrics.hot as f64;
        assert!(
            (edge_share - band_share).abs() <= 0.05,
            "{name}: {edge_share:.3} of the hot pixels in the edge band"
        );
        let mean_bin = metrics.hot as f64 / 64.0;
        assert!(
            metrics.fullest_bin as f64 <= 1.25 * mean_bin,
            "{name}: busiest cell {} against a mean {mean_bin:.0}",
            metrics.fullest_bin
        );
    }
}
