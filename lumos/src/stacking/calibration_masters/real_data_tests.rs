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
use common::file_utils::files_with_extensions;
use quickbench::quick_bench;

use crate::testing::{calibration_dir, init_tracing};
use crate::{
    CalibrationFrames, CalibrationMasters, CfaImage, DEFAULT_SIGMA_THRESHOLD, StackConfig,
};

use super::stack_cfa_master;

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
    let raf = |sub: &str| files_with_extensions(&dir.join(sub), &["raf"]);
    let paths = CalibrationPaths {
        darks: raf("Darks"),
        flats: raf("Flats"),
        bias: raf("Bias"),
    };
    if paths.darks.is_empty() || paths.flats.is_empty() || paths.bias.is_empty() {
        return None;
    }
    Some(paths)
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn builds_full_master_set() {
    init_tracing();
    let Some(paths) = calibration_paths() else {
        panic!("calibration frames missing — run scripts/fetch-test-data.sh");
    };

    let masters = CalibrationMasters::from_files(
        CalibrationFrames {
            darks: &paths.darks,
            flats: &paths.flats,
            bias: &paths.bias,
            flat_darks: &[],
        },
        DEFAULT_SIGMA_THRESHOLD,
    )
    .expect("master build failed");

    // Every supplied role yields a master; the un-supplied flat-dark stays `None`.
    let dark = masters.master_dark.as_ref().expect("master dark");
    let flat = masters.master_flat.as_ref().expect("master flat");
    let bias = masters.master_bias.as_ref().expect("master bias");
    assert!(masters.master_flat_dark.is_none());

    // All masters share the single sensor geometry (one CFA plane each).
    let (w, h) = (dark.data.width(), dark.data.height());
    assert!(w > 0 && h > 0, "degenerate master dimensions {w}x{h}");
    for (name, m) in [("flat", flat), ("bias", bias)] {
        assert_eq!(
            (m.data.width(), m.data.height()),
            (w, h),
            "{name} master dimensions differ from dark"
        );
    }

    // A defect map is derived whenever a dark or flat is present (hot from the dark,
    // cold from the flat), so the full set must produce one.
    assert!(
        masters.defect_map.is_some(),
        "defect map should be derived from dark + flat"
    );

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
    let dark_mean = mean_of(dark, "dark");
    let flat_mean = mean_of(flat, "flat");
    let bias_mean = mean_of(bias, "bias");

    // The flat is the one *illuminated* master, so it must be meaningfully brighter than the
    // unilluminated dark/bias (whose black-subtracted means sit near 0). This cross-checks that
    // each frame set was routed to the right master rather than swapped.
    assert!(
        flat_mean > 0.05,
        "flat master mean {flat_mean} too dark to be an illuminated flat"
    );
    assert!(
        flat_mean > dark_mean && flat_mean > bias_mean,
        "flat ({flat_mean}) should outshine dark ({dark_mean}) and bias ({bias_mean})"
    );
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
                CalibrationFrames {
                    darks: &paths.darks,
                    flats: &paths.flats,
                    bias: &paths.bias,
                    flat_darks: &[],
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
