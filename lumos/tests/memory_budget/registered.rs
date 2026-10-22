//! The registered pipeline: lights from files through
//! `AlignStackConfig::calibrate_align_stack_paths`, and lights held in memory through
//! `AlignStackConfig::align_stack_images`.

use lumos::{
    AlignStackConfig, CalibratedLights, CalibrationMasters, CosmicRayConfig, LinearImage,
    ProgressMonitor, Reference, RunReport,
};

use crate::alone::alone;
use crate::budget::Budget;
use crate::session::Session;
use crate::star_field::{Layout, StarField};

const SIDE: usize = 160;
const LIGHTS: usize = 4;
const STARS: usize = 40;
/// A budget the lights do not fit in that holds every stage's smallest step.
const SPILLED: Budget = Budget::kib(4608);

/// `AlignStackConfig::calibrate_align_stack_paths` of `lights` mosaics of `side` under `config` at
/// `budget`, the case named `label`.
#[derive(Debug)]
struct Mosaics<'a> {
    label: &'a str,
    side: usize,
    lights: usize,
    budget: Budget,
    config: &'a AlignStackConfig,
}

impl Mosaics<'_> {
    /// The run held to its budget once every light registered: its report.
    fn report(self) -> RunReport {
        let Self {
            label,
            side,
            lights,
            budget,
            config,
        } = self;
        let session = Session::new("lumos_budget_registered");
        let paths =
            StarField::new(side, STARS, 0x5EED).expose(session.scratch(), lights, Layout::Mosaic);
        let ingest = session.ingest(budget);
        let run = session.measure(|| {
            config
                .calibrate_align_stack_paths(
                    CalibratedLights {
                        paths: &paths,
                        masters: &CalibrationMasters::default(),
                    },
                    &ingest,
                    ProgressMonitor::unattended(),
                )
                .unwrap()
        });
        let result = run.value;
        assert_eq!(
            result.registered(),
            lights,
            "{label}: every light registers"
        );
        budget.assert_held(label, run.peak_bytes, &result.product.report);
        result.product.report
    }
}

/// The lights parked, the frames spilled and the overcommit a report states.
#[derive(Debug, PartialEq, Eq)]
struct Tiering {
    parked_lights: u64,
    spilled_frames: u64,
    overcommit_bytes: u64,
}

impl Tiering {
    const fn of(report: &RunReport) -> Self {
        Self {
            parked_lights: report.parked_lights,
            spilled_frames: report.spilled_frames,
            overcommit_bytes: report.overcommit_bytes,
        }
    }
}

/// With the reference found, every light is prepared and parked before any registers: on disk when
/// the set does not fit, and read back to be warped. The budget holds every stage's smallest step,
/// so the run reports no excess and keeps to the budget itself.
#[test]
fn a_spilled_run_that_finds_its_reference_stays_within_its_budget() {
    alone(|| {
        let report = Mosaics {
            label: "found, spilled",
            side: SIDE,
            lights: LIGHTS,
            budget: SPILLED,
            config: &AlignStackConfig::default(),
        }
        .report();
        assert_eq!(
            Tiering::of(&report),
            Tiering {
                parked_lights: LIGHTS as u64,
                spilled_frames: LIGHTS as u64,
                overcommit_bytes: 0,
            }
        );
    });
}

/// With the reference named, each light is prepared, registered and stored in one pass, and none
/// is parked.
#[test]
fn a_spilled_run_with_a_named_reference_stays_within_its_budget() {
    alone(|| {
        let config = AlignStackConfig {
            reference: Reference::Idx(1),
            ..AlignStackConfig::default()
        };
        let report = Mosaics {
            label: "named, spilled",
            side: SIDE,
            lights: LIGHTS,
            budget: SPILLED,
            config: &config,
        }
        .report();
        assert_eq!(
            Tiering::of(&report),
            Tiering {
                parked_lights: 0,
                spilled_frames: LIGHTS as u64,
                overcommit_bytes: 0,
            }
        );
    });
}

/// The cosmic-ray pass over each mosaic holds its masks and its scratch beside the mosaic: two
/// lights of 128², the pass being slow to test on more.
#[test]
fn a_spilled_run_cleaned_of_cosmic_rays_stays_within_its_budget() {
    alone(|| {
        let config = AlignStackConfig {
            cosmic_ray: Some(CosmicRayConfig::default()),
            ..AlignStackConfig::default()
        };
        let report = Mosaics {
            label: "cosmic rays, spilled",
            side: 128,
            lights: 2,
            budget: Budget::kib(2624),
            config: &config,
        }
        .report();
        assert_eq!(
            Tiering::of(&report),
            Tiering {
                parked_lights: 2,
                spilled_frames: 2,
                overcommit_bytes: 0,
            }
        );
    });
}

/// A set that fits stays resident, all of it inside the budget.
#[test]
fn a_resident_run_stays_within_its_budget() {
    alone(|| {
        let report = Mosaics {
            label: "found, resident",
            side: SIDE,
            lights: LIGHTS,
            budget: Budget::mib(32),
            config: &AlignStackConfig::default(),
        }
        .report();
        assert_eq!(
            Tiering::of(&report),
            Tiering {
                parked_lights: 0,
                spilled_frames: 0,
                overcommit_bytes: 0,
            }
        );
    });
}

/// Lights the caller decoded before the run were in memory before it read the machine: what the
/// run adds to them stays within its budget.
#[test]
fn held_lights_add_no_more_than_the_budget() {
    alone(|| {
        let session = Session::new("lumos_budget_held");
        let budget = Budget::mib(16);
        let ingest = session.ingest(budget);
        let field = StarField::new(SIDE, STARS, 0x5EED);
        let lights: Vec<LinearImage> = (0..LIGHTS).map(|idx| field.frame(idx)).collect();
        let run = session.measure(|| {
            AlignStackConfig::default()
                .align_stack_images(lights, &ingest, ProgressMonitor::unattended())
                .unwrap()
        });
        assert_eq!(run.value.registered(), LIGHTS);
        budget.assert_held("held", run.peak_bytes, &run.value.product.report);
    });
}
