//! Frames drizzled onto a finer grid and combined: `DrizzleConfig::drizzle_paths` of linear files.

use glam::DVec2;
use lumos::{
    DrizzleConfig, DrizzleFrame, Normalization, ProgressMonitor, StackConfig, Transform,
    WarpTransform,
};

use crate::alone::alone;
use crate::budget::Budget;
use crate::flat;
use crate::session::Session;
use crate::star_field::{Layout, StarField};

const SIDE: usize = 96;
const FRAMES: usize = 5;

/// [`FRAMES`] dithered frames drizzled at twice their scale, spilled at a budget they do not fit
/// in, stay within it.
#[test]
fn a_spilled_drizzle_stays_within_its_budget() {
    alone(|| {
        let session = Session::new("lumos_budget_drizzle");
        let frames: Vec<DrizzleFrame<_>> = StarField::new(SIDE, 20, 0xD1A2)
            .expose(session.scratch(), FRAMES, Layout::Mono)
            .into_iter()
            .enumerate()
            .map(|(idx, source)| {
                let [x, y] = StarField::dither(idx);
                DrizzleFrame {
                    source,
                    warp: WarpTransform::new(Transform::translation(DVec2::new(
                        f64::from(x),
                        f64::from(y),
                    ))),
                    pixel_weight_map: None,
                }
            })
            .collect();
        let budget = Budget::mib(3);
        let drizzle = DrizzleConfig {
            scale: 2.0,
            ..DrizzleConfig::default()
        };
        let ingest = session.ingest(budget);
        let run = session.measure(|| {
            drizzle
                .drizzle_paths(frames, &ingest, ProgressMonitor::unattended())
                .unwrap()
        });
        let report = &run.value.report;
        assert_eq!(report.spilled_frames, FRAMES as u64);
        budget.assert_held("drizzle", run.peak_bytes, report);
    });
}

/// [`FRAMES`] mosaics a flat divided, each carrying its gain, drizzled at twice their scale: each
/// drizzled frame keeps the gain warped onto the output grid. Resident, the run holds no more than
/// its plan expected; spilled, no more than its budget.
#[test]
fn a_flat_divided_drizzle_stays_within_its_budget() {
    alone(|| {
        // Built before the masters, whose build is the first to use the pool.
        let session = Session::new("lumos_budget_flat_drizzle");
        let masters = flat::masters(SIDE);
        let field = StarField::new(SIDE, 20, 0xD1A2);
        for (label, budget, spilled) in [
            ("flat drizzle, resident", Budget::mib(64), 0),
            ("flat drizzle, spilled", Budget::mib(4), FRAMES as u64),
        ] {
            let frames: Vec<DrizzleFrame<_>> = (0..FRAMES)
                .map(|idx| {
                    let mut source = field.mosaic(idx);
                    masters.calibrate(&mut source).unwrap();
                    let [x, y] = StarField::dither(idx);
                    DrizzleFrame {
                        source,
                        warp: WarpTransform::new(Transform::translation(DVec2::new(
                            f64::from(x),
                            f64::from(y),
                        ))),
                        pixel_weight_map: None,
                    }
                })
                .collect();
            // A few stars over little sky give a color too faint a spread to normalize by, which
            // the memory under test does not need.
            let drizzle = DrizzleConfig {
                scale: 2.0,
                stack: StackConfig {
                    normalization: Normalization::None,
                    ..StackConfig::light()
                },
                ..DrizzleConfig::default()
            };
            let ingest = session.ingest(budget);
            let run = session.measure(|| {
                drizzle
                    .drizzle_cfa_images(frames, &ingest, ProgressMonitor::unattended())
                    .unwrap()
            });
            let report = &run.value.report;
            assert_eq!(report.spilled_frames, spilled, "{label}");
            budget.assert_held(label, run.peak_bytes, report);
        }
    });
}
