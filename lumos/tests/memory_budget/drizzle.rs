//! Frames drizzled onto a finer grid and combined: `DrizzleConfig::drizzle_paths` of linear files.

use glam::DVec2;
use lumos::{DrizzleConfig, DrizzleFrame, ProgressMonitor, Transform, WarpTransform};

use crate::alone::alone;
use crate::budget::Budget;
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
            .map(|(index, source)| {
                let [x, y] = StarField::dither(index);
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
