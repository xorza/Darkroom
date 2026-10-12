//! The registered pipeline: lights from files through
//! `AlignStackConfig::calibrate_align_stack_paths`, and lights held in memory through
//! `AlignStackConfig::align_stack_images`.

use common::CancelToken;
use lumos::{
    AlignStackConfig, CalibrationMasters, CosmicRayConfig, LinearImage, ProgressCallback,
    Reference, RunReport,
};

use crate::budget::Budget;
use crate::session::Session;
use crate::star_field::{Layout, StarField};

const SIDE: usize = 160;
const LIGHTS: usize = 4;
const STARS: usize = 40;
/// A budget the lights do not fit in that holds every stage's smallest step.
const SPILLED: Budget = Budget::kib(4608);

/// `AlignStackConfig::calibrate_align_stack_paths` of `lights` mosaics of `side` under `config` at
/// `budget`, held to it once every light registered: its report.
fn mosaics(
    label: &str,
    side: usize,
    lights: usize,
    budget: Budget,
    config: &AlignStackConfig,
) -> RunReport {
    let session = Session::new("lumos_budget_registered");
    let paths =
        StarField::new(side, STARS, 0x5EED).expose(session.scratch(), lights, Layout::Mosaic);
    let ingest = session.ingest(budget);
    let run = session.measure(|| {
        config
            .calibrate_align_stack_paths(
                &paths,
                &CalibrationMasters::default(),
                &ingest,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap()
    });
    let result = run.value;
    assert_eq!(
        result.alignment.registered(),
        lights,
        "{label}: every light registers"
    );
    budget.assert_held(label, run.peak_bytes, &result.product.report);
    result.product.report
}

/// The lights parked, the frames spilled and the overcommit a report states.
const fn tiering(report: &RunReport) -> (u64, u64, u64) {
    (
        report.parked_lights,
        report.spilled_frames,
        report.overcommit_bytes,
    )
}

/// With the reference found, every light is prepared and parked before any registers: on disk when
/// the set does not fit, and read back to be warped. The budget holds every stage's smallest step,
/// so the run reports no excess and keeps to the budget itself.
#[test]
fn a_spilled_run_that_finds_its_reference_stays_within_its_budget() {
    let report = mosaics(
        "found, spilled",
        SIDE,
        LIGHTS,
        SPILLED,
        &AlignStackConfig::default(),
    );
    assert_eq!(tiering(&report), (LIGHTS as u64, LIGHTS as u64, 0));
}

/// With the reference named, each light is prepared, registered and stored in one pass, and none
/// is parked.
#[test]
fn a_spilled_run_with_a_named_reference_stays_within_its_budget() {
    let config = AlignStackConfig {
        reference: Reference::Index(1),
        ..AlignStackConfig::default()
    };
    let report = mosaics("named, spilled", SIDE, LIGHTS, SPILLED, &config);
    assert_eq!(tiering(&report), (0, LIGHTS as u64, 0));
}

/// The cosmic-ray pass over each mosaic holds its masks and its scratch beside the mosaic: two
/// lights of 128², the pass being slow to test on more.
#[test]
fn a_spilled_run_cleaned_of_cosmic_rays_stays_within_its_budget() {
    let config = AlignStackConfig {
        cosmic_ray: Some(CosmicRayConfig::default()),
        ..AlignStackConfig::default()
    };
    let report = mosaics("cosmic rays, spilled", 128, 2, Budget::kib(2624), &config);
    assert_eq!(tiering(&report), (2, 2, 0));
}

/// A set that fits stays resident, all of it inside the budget.
#[test]
fn a_resident_run_stays_within_its_budget() {
    let report = mosaics(
        "found, resident",
        SIDE,
        LIGHTS,
        Budget::mib(32),
        &AlignStackConfig::default(),
    );
    assert_eq!(tiering(&report), (0, 0, 0));
}

/// Lights the caller decoded before the run were in memory before it read the machine: what the
/// run adds to them stays within its budget.
#[test]
fn held_lights_add_no_more_than_the_budget() {
    let session = Session::new("lumos_budget_held");
    let budget = Budget::mib(16);
    let ingest = session.ingest(budget);
    let field = StarField::new(SIDE, STARS, 0x5EED);
    let lights: Vec<LinearImage> = (0..LIGHTS).map(|index| field.frame(index)).collect();
    let run = session.measure(|| {
        AlignStackConfig::default()
            .align_stack_images(
                lights,
                &ingest,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap()
    });
    assert_eq!(run.value.alignment.registered(), LIGHTS);
    budget.assert_held("held", run.peak_bytes, &run.value.product.report);
}
