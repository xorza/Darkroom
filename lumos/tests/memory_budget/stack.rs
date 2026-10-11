//! Frames decoded straight into a combine: `stack` of linear files, and the calibration masters
//! `CalibrationMasters::build` stacks from mosaics.

use std::path::PathBuf;

use common::CancelToken;
use lumos::{
    CalibrationMasters, CalibrationSet, DEFAULT_SIGMA_THRESHOLD, NoMasterCache, ProgressCallback,
    RunReport, StackConfig, stack,
};

use crate::budget::Budget;
use crate::session::Session;
use crate::star_field::{Layout, StarField};

const SIDE: usize = 160;
const FRAMES: usize = 6;

/// [`FRAMES`] linear frames stacked at a budget, spilled when they do not fit and resident when
/// they do, each within its budget and what it reports past it.
///
/// The spilled budget, 1 572 864 B, is less than one frame takes to measure: its 102 400 B of
/// samples and 25 600 of flags, and the MRS estimate of its noise over one tile that is the frame,
/// 25 600 coefficients of 33 bytes, 32 bytes of transform a pixel and 88 of bookkeeping,
/// 1 664 088 B. The run reports the 219 224 B those 1 792 088 B are past it.
#[test]
fn a_stack_stays_within_its_budget() {
    let session = Session::new("lumos_budget_stack");
    let paths = StarField::new(SIDE, 30, 0x57AC).expose(session.scratch(), FRAMES, Layout::Mono);
    for (label, budget, spilled, overcommit) in [
        ("spilled", Budget::mib(2), FRAMES as u64, 219_224),
        ("resident", Budget::mib(16), 0, 0),
    ] {
        let ingest = session.ingest(budget);
        let run = session.measure(|| {
            stack(
                &paths,
                &StackConfig::light(),
                &ingest,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap()
        });
        let report = &run.value.report;
        assert_eq!(
            (report.spilled_frames, report.overcommit_bytes),
            (spilled, overcommit),
            "{label}"
        );
        budget.assert_held(label, run.peak_bytes, report);
    }
}

/// A master dark stacked from [`FRAMES`] mosaics stays within a budget that holds every stage's
/// smallest step, whichever tier its frames take: the masters report no overcommit to allow for.
#[test]
fn a_master_stays_within_its_budget() {
    let session = Session::new("lumos_budget_master");
    let darks = StarField::new(SIDE, 0, 0xDA4C).expose(session.scratch(), FRAMES, Layout::Mosaic);
    let budget = Budget::mib(2);
    let ingest = session.ingest(budget);
    let none: &[PathBuf] = &[];
    let run = session.measure(|| {
        CalibrationMasters::build(
            CalibrationSet {
                dark: darks.as_slice(),
                flat: none,
                bias: none,
                flat_dark: none,
            },
            DEFAULT_SIGMA_THRESHOLD,
            &ingest,
            &NoMasterCache,
            &ProgressCallback::default(),
            &CancelToken::never(),
        )
        .unwrap()
    });
    budget.assert_held("master dark", run.peak_bytes, &RunReport::default());
}
