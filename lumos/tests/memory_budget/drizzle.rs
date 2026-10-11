//! Frames drizzled onto a finer grid and combined: `drizzle_stack` of linear files.

use common::CancelToken;
use glam::DVec2;
use lumos::{
    DrizzleConfig, DrizzleFrame, ProgressCallback, StackConfig, Transform, WarpTransform,
    drizzle_stack,
};

use crate::budget::Budget;
use crate::session::Session;
use crate::star_field::{Layout, StarField};

const SIDE: usize = 96;
const FRAMES: usize = 5;

/// [`FRAMES`] dithered frames drizzled at twice their scale, spilled at a budget they do not fit
/// in, stay within it.
#[test]
fn a_spilled_drizzle_stays_within_its_budget() {
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
        ingest: session.ingest(budget),
        ..DrizzleConfig::default()
    };
    let run = session.measure(|| {
        drizzle_stack(
            frames,
            &drizzle,
            &StackConfig::light(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
    });
    let report = &run.value.product.report;
    assert_eq!(report.spilled_frames, FRAMES as u64);
    budget.assert_held("drizzle", run.peak_bytes, report);
}
