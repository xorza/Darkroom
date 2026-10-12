use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::bayer::rcd;
use crate::image::cfa::demosaic::xtrans::markesteijn;
use crate::image::image_dimensions::ImageDimensions;
use crate::internals::cfa::XTRANS_PATTERN;
use crate::memory::budget::Budget;
use crate::memory::footprint::Footprint;
use crate::memory::run_plan::RunPlan;
use crate::memory::stage::{MIN_CHUNK_ROWS, Stage, Units, Width};
use crate::memory::*;
use crate::platform::SystemMemory;
use crate::stack_product::quality_planes::QualityPlanes;

const MIB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MIB;
/// The detector's planes these plans are written for. The planner's arithmetic is what they pin;
/// the detector's own count is pinned against the planner in
/// `star_detection::tests::pool_working_set`.
const DETECTOR_PLANES: usize = 7;
const FRAME_96MB: usize = 6240 * 4160 * size_of::<f32>();

/// The stages of [`found_reference`], by index.
const PREPARE: usize = 0;
const WARP: usize = 1;

/// Each reading takes a system of its own, so the second must be as good as the first — a stale
/// or half-refreshed reading would show up as a zero or as a value that stops tracking.
#[test]
fn available_memory_reports_a_plausible_figure_on_every_call() {
    let first = SystemMemory::available();
    let second = SystemMemory::available();

    assert!(first > 0, "no available memory reported");
    assert!(second > 0, "second call reported none");
    // Both are sampled within microseconds of each other on an otherwise-idle test process, so
    // they cannot differ by more than a small fraction without the refresh being broken.
    let (low, high) = (first.min(second), first.max(second));
    assert!(
        high - low < high / 4,
        "consecutive samples disagree wildly: {first} then {second}"
    );
    // A budget derived from it stays inside the machine, which a garbage reading would not.
    assert!(memory_budget(first) < first);
}

#[test]
fn memory_budget_keeps_one_quarter_as_headroom_without_overflow() {
    assert_eq!(
        memory_budget(8 * 1024 * 1024 * 1024),
        6 * 1024 * 1024 * 1024
    );
    assert_eq!(memory_budget(u64::MAX), 13_835_058_055_282_163_711);
}

/// Each stage's width by hand. `FRAME_96MB` is 99.02 MiB, so a transient of two is 198.05 MiB.
/// - 20 736 MiB usable: 20 resident frames take 1980.5, leaving 18 755.5 for 94 transients: the 16
///   workers bind.
/// - 19 200 MiB usable is less than 200 resident frames' 19 804.7: no room, pinned to 1, and the
///   200 frames and the one transient, 20 974 387 200 B, are 841 728 000 B past it.
/// - 3 GiB: three 1 GiB transients, one 2 GiB one, and fifteen of the 96 MB frame's, of which the
///   8 workers bind. 1.5 GiB: one 1 GiB transient. 6 GiB: six.
/// - Spilled, a frame in work holds itself too: 1 GiB kept and 1 GiB in flight are 2 GiB a unit,
///   three in 6 GiB.
/// - Nothing usable: still one, 1 GiB past the budget, so the run makes progress.
/// - A stage one frame at a time runs one, and a stage with no work of its own holds its own bytes
///   beside the set and nothing more.
#[test]
fn a_stage_runs_as_wide_as_its_budget_leaves() {
    let gib = GB as usize;
    let frames = |kept, in_flight| Stage {
        kept,
        held: 0,
        in_flight,
        units: Units::Frames,
    };
    for (stage, set, usable, workers, spilled, expected) in [
        (
            frames(FRAME_96MB, 2 * FRAME_96MB),
            20 * FRAME_96MB as u64,
            20_736 * MIB,
            16,
            false,
            (16, 0),
        ),
        (
            frames(FRAME_96MB, 2 * FRAME_96MB),
            200 * FRAME_96MB as u64,
            19_200 * MIB,
            16,
            false,
            (1, 841_728_000),
        ),
        (frames(gib, gib), 0, 3 * GB, 64, false, (3, 0)),
        (frames(gib, 2 * gib), 0, 3 * GB, 64, false, (1, 0)),
        (
            frames(FRAME_96MB, 2 * FRAME_96MB),
            0,
            3 * GB,
            8,
            false,
            (8, 0),
        ),
        (frames(gib, gib), 0, 3 * GB / 2, 16, false, (1, 0)),
        (frames(gib, gib), 0, 6 * GB, 16, false, (6, 0)),
        (frames(gib, gib), 0, 6 * GB, 16, true, (3, 0)),
        (frames(gib, gib), 0, 0, 16, false, (1, GB)),
        (
            Stage {
                units: Units::Serial,
                ..frames(gib, gib)
            },
            0,
            6 * GB,
            16,
            false,
            (1, 0),
        ),
        (
            Stage {
                held: 2 * gib,
                units: Units::Whole,
                ..frames(gib, gib)
            },
            0,
            GB,
            16,
            true,
            (1, GB),
        ),
    ] {
        let width = stage.width(Budget::new(usable, workers), 1000, set, spilled);
        assert_eq!(
            (width.units, width.overcommit_bytes),
            expected,
            "{stage:?} beside {set} B, {usable} B, {workers} workers, spilled {spilled}"
        );
    }
}

/// A stack decoded straight into the combine fits when its frames, one decode's statistics scratch
/// and the combine's resident output all fit. Ten 4 MiB frames are 40 MiB, one scratch frame
/// makes the decode peak 44, and two output planes make the combine peak 48: 48 MiB usable is
/// the boundary, and output planes the frames alone leave room for still spill the run. The
/// normalization is done before the output is allocated, so the larger of the two counts: two
/// planes of it beside two of output fit as two of output alone, a third does not.
///
/// A masked frame's quality planes are resident, the scratch is one plain frame on top: 30 MiB
/// resident, 31 MiB at the decode's peak.
#[test]
fn a_decoded_stack_charges_its_frames_scratch_and_output() {
    let frame = plane(4);
    let stack = |kept, scratch, output_planes: usize, normalization_planes: usize| {
        decoded_stack(
            kept,
            scratch,
            output_planes * frame,
            normalization_planes * frame,
        )
    };
    let resident =
        |stages: &[Stage], usable| RunPlan::of(10, stages, Budget::new(usable, 8)).resident;
    let boundary = 48 * MIB;
    assert!(resident(&stack(frame, frame, 2, 0), boundary));
    assert!(!resident(&stack(frame, frame, 2, 0), boundary - 1));
    assert!(resident(&stack(frame, frame, 0, 0), boundary - 1));
    assert!(!resident(&stack(frame, frame, 3, 0), boundary));
    assert!(resident(&stack(frame, frame, 2, 2), boundary));
    assert!(!resident(&stack(frame, frame, 2, 3), boundary));

    let masked = stack(3 * plane(1), plane(1), 0, 0);
    assert!(resident(&masked, 31 * MIB));
    assert!(!resident(&masked, 31 * MIB - 1));

    // Saturating arithmetic: a frame count no budget can hold never wraps into "fits".
    assert!(
        !RunPlan::of(
            2,
            &stack(usize::MAX, 1, 0, 0),
            Budget::new(memory_budget(u64::MAX), 8)
        )
        .resident
    );
}

/// The combine's output planes are charged beside the warped frames, not only the decode and
/// warp peaks: at `QualityPlanes::STANDARD` an RGB output is 3 × (image, weight, variance) +
/// coverage = 10 planes and a flag byte a pixel, ¼ plane, which flips a set the warp stage alone
/// would keep in RAM.
///
/// Mono, five frames, one worker: the decode peaks at 5 frames + its 1P statistics copy and the 7P
/// detector = 13P; the warp at 5 × 3¼P warped + its 1P source = 17¼P; and the combine at 16¼P + the
/// output's 10¼P = 26½P, which decides. In quarter planes: 106 and 69.
#[test]
fn a_warped_run_charges_the_combine_output() {
    let plane_bytes = plane(10);
    let output = QualityPlanes::STANDARD.heap_bytes(ImageDimensions::new(
        ((10 * MIB) as usize / size_of::<f32>(), 1),
        3,
    ));
    assert_eq!(output, 10 * plane_bytes + plane_bytes / 4);
    let resident = |output_bytes, usable| {
        RunPlan::of(
            5,
            &found_reference(plane_bytes, mono(plane_bytes), output_bytes),
            Budget::new(usable, 1),
        )
        .resident
    };
    let quarter = plane_bytes as u64 / 4;
    assert!(resident(output, 106 * quarter));
    assert!(!resident(output, 106 * quarter - 1));
    assert!(resident(0, 69 * quarter));
    assert!(!resident(0, 69 * quarter - 1));
}

/// 30 RGB frames of 24 MP that the caller holds, eight workers: planes of 96 MB, frames of 288 MB.
/// The reading already left the 30 × 288 MB = 8.64 GB of inputs out. A warped frame is 288 + 2 ×
/// 96 + 24 = 504 MB, less the 288 MB source it frees: 30 × 216 MB = 6.48 GB, plus one source in
/// work, 288 MB, is the warp's 6.768 GB floor. The decode keeps nothing the caller did not hold and
/// adds the statistics copy and the detector, 288 + 672 = 960 MB. The run stays resident at a
/// budget of exactly 6.768 GB, seven decodes and one warp wide, and spills a byte below it. Charged
/// as frames the run decodes, the warp needs 30 × 504 MB + 288 MB = 15.408 GB.
#[test]
fn held_frames_are_charged_only_what_the_run_adds() {
    const MB: usize = 1_000_000;
    let (plane_bytes, frame_bytes) = (96 * MB, 288 * MB);
    let decode = Footprint {
        kept: frame_bytes,
        peak: 2 * frame_bytes,
    };
    let held = |held: usize| {
        let mut stages = found_reference(plane_bytes, decode, 0);
        for stage in &mut stages {
            stage.kept -= held;
        }
        stages
    };
    assert_eq!(warped(plane_bytes, frame_bytes), 504 * MB);
    let peak = 6_768 * MB as u64;
    let plan = RunPlan::of(30, &held(frame_bytes), Budget::new(peak, 8));
    assert!(plan.resident);
    assert_eq!((plan.width(PREPARE), plan.width(WARP)), (7, 1));
    assert!(!RunPlan::of(30, &held(frame_bytes), Budget::new(peak - 1, 8)).resident);
    assert!(!RunPlan::of(30, &held(0), Budget::new(peak, 8)).resident);
    assert!(RunPlan::of(30, &held(0), Budget::new(15_408 * MB as u64, 8)).resident);
}

/// One pass over ten frames of 10 MiB planes, eight threads: a decode peaks at 7P, the
/// detector holds 7P, and a warped frame is 5¼P. A worker holds 14P, and on the spill tier its 5¼P
/// warp buffers too. Resident needs the 52½P warped set beside one worker, 66½P, where one worker
/// fits; at 164½P all eight do. A byte below 66½P the run spills, and three 19¼P workers fit; at
/// 10P not even one does, and it holds 9¼P past the budget. A normalization of 20P beside the
/// warped set, 72½P, keeps the run off the resident tier at 66½P; one of 30P on the spill tier
/// holds 20P past a 10P budget.
#[test]
fn a_single_pass_holds_the_warped_set_beside_its_workers() {
    let plane_bytes = plane(10);
    let quarter = plane_bytes as u64 / 4;
    let pass = |normalization_planes: usize| {
        let decode = three_channel(plane_bytes, 7).beside(3 * plane_bytes);
        let warped = warped(plane_bytes, 3 * plane_bytes);
        [
            Stage {
                kept: warped,
                held: 0,
                in_flight: decode.peak + DETECTOR_PLANES * plane_bytes,
                units: Units::Frames,
            },
            normalize(warped, normalization_planes * plane_bytes),
            combine(warped, 0),
        ]
    };
    let at = |stages: &[Stage], quarters: u64| {
        RunPlan::of(10, stages, Budget::new(quarters * quarter, 8))
    };
    let width = |plan: RunPlan| (plan.resident, plan.width(0));
    assert_eq!(width(at(&pass(0), 266)), (true, 1));
    assert_eq!(width(at(&pass(0), 658)), (true, 8));
    assert_eq!(width(at(&pass(0), 265)), (false, 3));
    assert_eq!(at(&pass(0), 266).prior_overcommit(), 0);
    assert_eq!(at(&pass(0), 40).prior_overcommit(), 37 * quarter);

    assert!(!at(&pass(20), 266).resident);
    assert!(at(&pass(20), 290).resident);
    assert_eq!(at(&pass(30), 40).prior_overcommit(), 80 * quarter);
}

/// The combine's rows by hand, spilled: the budget left after the planes it holds, over the bytes a
/// row of every input plane costs, floored at `MIN_CHUNK_ROWS` unless the image has fewer, and what
/// the floor holds past the budget.
/// - 6000 px × 60 planes (3 channels × 20 frames) × 4 B = 1 440 000 B a row: 6 GiB is 4473.9 rows,
///   768 MiB is 559.2, 192 MiB is 139.8, of the image's 20 000; of 100, all 100.
/// - 6000 px × 20 mono planes = 480 000 B a row: 6 GiB is 13 421.8 rows.
/// - Nothing usable: the floor, 64 rows of 800 B past it.
/// - 786 432 B usable; six resident 100×200 planes take 480 000, and nine input planes cost 3600 B a
///   row, so 85 whole rows fit. A resident flag plane adds 20 000 B, which leaves 79; 108 000 B the
///   run holds beside the pass leave 198 432 B, 55 rows, under the floor: 64 rows of 3600 B,
///   480 000 + 108 000 + 230 400 − 786 432 = 31 968 B over.
/// - Ten resident planes take 800 000 B, past the budget: the floor, holding 64 rows of 3600 B
///   beside them, 800 000 + 230 400 − 786 432 = 243 968 B over.
/// - A 30-row image whose 60 resident planes take 720 000 B fits 18 rows; under the floor it holds
///   its 30 rows, 720 000 + 108 000 − 786 432 = 41 568 B over.
/// - A row of no pixels costs nothing: every row at once.
#[test]
fn combine_rows_match_budget_arithmetic() {
    let rows = |input_planes: usize,
                resident_planes: usize,
                flags: usize,
                beside: usize,
                width: usize,
                height: usize| {
        Stage {
            kept: 0,
            held: (resident_planes * size_of::<f32>() + flags) * width * height + beside,
            in_flight: input_planes * size_of::<f32>() * width,
            units: Units::Rows(height),
        }
    };
    let fits = |units| Width {
        units,
        overcommit_bytes: 0,
    };
    let floor = |overcommit_bytes| Width {
        units: MIN_CHUNK_ROWS,
        overcommit_bytes,
    };
    for (stage, usable, expected) in [
        (rows(60, 0, 0, 0, 6000, 20_000), 6 * GB, fits(4473)),
        (rows(60, 0, 0, 0, 6000, 20_000), 768 * MIB, fits(559)),
        (rows(60, 0, 0, 0, 6000, 20_000), 192 * MIB, fits(139)),
        (rows(60, 0, 0, 0, 6000, 100), 6 * GB, fits(100)),
        (rows(20, 0, 0, 0, 6000, 20_000), 6 * GB, fits(13_421)),
        (rows(2, 0, 0, 0, 100, 100), 0, floor(64 * 800)),
        (rows(9, 6, 0, 0, 100, 200), 786_432, fits(85)),
        (rows(9, 6, 1, 0, 100, 200), 786_432, fits(79)),
        (rows(9, 6, 0, 108_000, 100, 200), 786_432, floor(31_968)),
        (rows(9, 10, 0, 0, 100, 200), 786_432, floor(243_968)),
        (
            rows(9, 60, 0, 0, 100, 30),
            786_432,
            Width {
                units: 30,
                overcommit_bytes: 41_568,
            },
        ),
        (rows(60, 3, 0, 0, 0, 100), 6 * GB, fits(100)),
    ] {
        assert_eq!(
            stage.width(Budget::new(usable, 1), 1, 0, true),
            expected,
            "{stage:?} at {usable} B"
        );
    }
}

/// The planes the boundary arithmetic below is written in: a mono frame keeps and peaks at its one
/// plane. Both demosaics keep 3 and peak at 4 (the frame and the output) and the tile buffers of
/// each of the run's workers.
#[test]
fn demosaic_costs_in_planes() {
    let plane_bytes = plane(10);
    assert_eq!(mono(plane_bytes), Footprint::kept(plane_bytes));
    for (cfa_type, workspace) in [
        (
            CfaType::Bayer(CfaPattern::Rggb),
            rcd::internals::workspace_bytes(4),
        ),
        (
            CfaType::XTrans(XTRANS_PATTERN),
            markesteijn::internals::workspace_bytes(4),
        ),
    ] {
        assert_eq!(
            demosaic(cfa_type, plane_bytes),
            Footprint {
                kept: 3 * plane_bytes,
                peak: 4 * plane_bytes + workspace,
            }
        );
    }
    // A warped frame is its output, two quality planes and a quarter-plane of flags: 3¼P mono,
    // 5¼P colour, 13 and 21 quarter planes.
    let quarter = plane_bytes / 4;
    assert_eq!(warped(plane_bytes, plane_bytes), 13 * quarter);
    assert_eq!(warped(plane_bytes, 3 * plane_bytes), 21 * quarter);
}

/// 100 MiB planes, ten frames of a 22P decode, eight workers, 61.44P usable. The warped set is
/// 52½P and one warp's 3P source beside it 55½P; the decode pass 30P + one 26P transient, 56P:
/// the run stays resident, one decode wide in the 31.44P beside the frames, and two warps wide in
/// the 8.94P beside the warped set.
#[test]
fn a_set_whose_warped_frames_fit_stays_resident_and_narrows_its_warp() {
    let plane_bytes = plane(100);
    let plan = RunPlan::of(
        10,
        &found_reference(plane_bytes, three_channel(plane_bytes, 22), 0),
        Budget::new(memory_budget(8 * GB), 8),
    );
    assert!(plan.resident);
    assert_eq!((plan.width(PREPARE), plan.width(WARP)), (1, 2));
}

/// 100 MiB planes, ten frames, eight workers, 61.44P usable; each run stays resident.
/// - Mono: the decode pass is 10P + 8P (its 1P statistics copy and the 7P detector), the warp
///   32½P + 1P. Decodes take 8P each from the 51.44P beyond the frames: six. The warp's 1P sources
///   fit all eight workers in the 28.94P beyond the warped set.
/// - The 7P decode: 30P + 11P; its warp 52½P + 3P. Decodes take 11P each of 31.44P: two. Warps
///   take 3P each of 8.94P: two.
/// - The 22P decode: 26P a decode, one; warps as the 7P one's, two.
#[test]
fn widths_follow_each_demosaics_peak() {
    let plane_bytes = plane(100);
    for (decode, expected) in [
        (mono(plane_bytes), (6, 8)),
        (three_channel(plane_bytes, 7), (2, 2)),
        (three_channel(plane_bytes, 22), (1, 2)),
    ] {
        let plan = RunPlan::of(
            10,
            &found_reference(plane_bytes, decode, 0),
            Budget::new(memory_budget(8 * GB), 8),
        );
        assert!(plan.resident, "{decode:?}");
        assert_eq!(
            (plan.width(PREPARE), plan.width(WARP)),
            expected,
            "{decode:?}"
        );
    }
}

/// 10 MiB planes, five frames of a 22P decode, eight workers, 614.4P usable: the decode pass is 15P
/// + 26P and the warp 26¼P + 3P, so it fits, and the frame count binds both widths.
#[test]
fn small_set_uses_all_workers_in_ram() {
    let plane_bytes = plane(10);
    let plan = RunPlan::of(
        5,
        &found_reference(plane_bytes, three_channel(plane_bytes, 22), 0),
        Budget::new(memory_budget(8 * GB), 8),
    );
    assert!(plan.resident);
    assert_eq!((plan.width(PREPARE), plan.width(WARP)), (5, 5));
}

/// 10 MiB planes, five frames, four workers. Each demosaic's RAM-tier boundary is its larger floor:
/// the 22P decode's pass, 5 × 3P + 26P = 41P, above its warp's 5 × 5¼P + 3P = 29¼P; the 7P one's
/// warp, 29¼P, above its 26P decode pass; mono's warp, 5 × 3¼P + 1P = 17¼P, above its 13P decode
/// pass. In quarter planes: 164, 117 and 69.
///
/// At 41P all three fit, and their decode transients buy different widths from what the resident
/// outputs leave: the 22P decode's 26P one of 26P, the 7P one's 11P two, mono's 8P four of 36P.
/// Room scales the 22P decode's width: 67P leaves 52P, two transients; 93P leaves 78P, three.
#[test]
fn ram_tier_respects_algorithm_specific_boundaries() {
    let plane_bytes = plane(10);
    let quarter = plane_bytes as u64 / 4;
    let plan = |decode, usable| {
        RunPlan::of(
            5,
            &found_reference(plane_bytes, decode, 0),
            Budget::new(usable, 4),
        )
    };
    for (decode, boundary) in [
        (three_channel(plane_bytes, 22), 164),
        (three_channel(plane_bytes, 7), 117),
        (mono(plane_bytes), 69),
    ] {
        assert!(plan(decode, boundary * quarter).resident, "{decode:?}");
        assert!(!plan(decode, boundary * quarter - 1).resident, "{decode:?}");
    }
    for (decode, width) in [
        (three_channel(plane_bytes, 22), 1),
        (three_channel(plane_bytes, 7), 2),
        (mono(plane_bytes), 4),
    ] {
        assert_eq!(
            plan(decode, 164 * quarter).width(PREPARE),
            width,
            "{decode:?}"
        );
    }
    for (planes, width) in [(67, 2), (93, 3)] {
        assert_eq!(
            plan(three_channel(plane_bytes, 22), planes * 4 * quarter).width(PREPARE),
            width
        );
    }
}

/// For every frame size, count, worker count and budget, each stage's width keeps its projected
/// peak — what the tier keeps beside it plus `width ×` what one unit in work adds — within the
/// budget, unless not even its floor fits, which the width then reports as its overcommit. Swept for
/// the pipeline's stages over each demosaic and the fixed decodes, and for stacks decoded straight
/// into the combine.
#[test]
fn a_plan_never_overshoots_its_budget() {
    for &plane_mib in &[16u64, 64, 100, 400] {
        let plane_bytes = plane(plane_mib);
        for &frames in &[4usize, 12, 30, 60] {
            let runs: Vec<Vec<Stage>> = [
                mono(plane_bytes),
                demosaic(CfaType::Bayer(CfaPattern::Rggb), plane_bytes),
                demosaic(CfaType::XTrans(XTRANS_PATTERN), plane_bytes),
                three_channel(plane_bytes, 7),
                three_channel(plane_bytes, 22),
            ]
            .map(|decode| found_reference(plane_bytes, decode, 0).to_vec())
            .into_iter()
            .chain([decoded_stack(plane_bytes, plane_bytes, 0, 0).to_vec()])
            .collect();
            for stages in &runs {
                for &workers in &[1usize, 8, 32] {
                    for &budget_gib in &[1u64, 2, 4, 8, 16] {
                        let budget = Budget::new(memory_budget(budget_gib * GB), workers);
                        let plan = RunPlan::of(frames, stages, budget);
                        let mut before = 0;
                        for (index, stage) in stages.iter().enumerate() {
                            let width = plan.width(index);
                            let set = if plan.resident {
                                before.max(stage.kept) as u64 * frames as u64
                            } else {
                                0
                            };
                            before = stage.kept;
                            let per_unit = match (stage.units, plan.resident) {
                                (Units::Whole, _) | (Units::Rows(_), true) => 0,
                                (Units::Frames | Units::Serial, false) => {
                                    stage.in_flight + stage.kept
                                }
                                _ => stage.in_flight,
                            } as u64;
                            if stage.units == Units::Frames {
                                assert!(width >= 1 && width <= workers.min(frames));
                            }
                            let projected = set + stage.held as u64 + width as u64 * per_unit;
                            assert!(
                                projected <= budget.usable() || width <= MIN_CHUNK_ROWS,
                                "stage {index}: {} MiB exceeds {} MiB usable",
                                projected / MIB,
                                budget.usable() / MIB
                            );
                        }
                    }
                }
            }
        }
    }
}

/// 100 MiB planes, twenty frames of a 22P decode, sixteen workers.
/// - 15.36P usable: spilled, under one 29P decode and two 8¼P warps; both pinned or bound to 1, the
///   decode 13.64P past the budget, 1364 MiB.
/// - 122.88P: the warp, 105P + 3P, and the decode, 60P + 26P, fit; the decode is two wide in the
///   62.88P beside the frames, the warp five in the 17.88P beside the warped set.
/// - 2⁵⁰ B: everything fits and the workers bind.
#[test]
fn budget_flips_the_tier_and_scales_the_widths() {
    let plane_bytes = plane(100);
    let stages = found_reference(plane_bytes, three_channel(plane_bytes, 22), 0);
    for (usable, resident, widths, overcommit) in [
        (memory_budget(2 * GB), false, (1, 1), 1364 * MIB),
        (memory_budget(16 * GB), true, (2, 5), 0),
        (1 << 50, true, (16, 16), 0),
    ] {
        let plan = RunPlan::of(20, &stages, Budget::new(usable, 16));
        assert_eq!(
            (
                plan.resident,
                (plan.width(PREPARE), plan.width(WARP)),
                plan.prior_overcommit()
            ),
            (resident, widths, overcommit),
            "{usable} B"
        );
    }
}

fn plane(mib: u64) -> usize {
    (mib * MIB) as usize
}

/// What each demosaic holds for a frame whose planes are `plane_bytes`, from the demosaics' own
/// accounting, on a one-row frame and four workers.
fn demosaic(cfa_type: CfaType, plane_bytes: usize) -> Footprint {
    cfa_type.demosaic_footprint(
        ImageDimensions::new((plane_bytes / size_of::<f32>(), 1), 1),
        4,
    )
}

fn mono(plane_bytes: usize) -> Footprint {
    demosaic(CfaType::Mono, plane_bytes)
}

/// A three-channel decode that peaks at `planes` planes. Fixed, where the demosaics' own peaks
/// carry the workers' tile buffers: 7 is the planner under a decode heavier than its output and its
/// statistics, 22 under one far heavier.
const fn three_channel(plane_bytes: usize, planes: usize) -> Footprint {
    Footprint {
        kept: 3 * plane_bytes,
        peak: planes * plane_bytes,
    }
}

/// A warped frame of `output_bytes` whose planes are `plane_bytes`: its output, its coverage and
/// confidence, and a flag byte a pixel; no gain.
const fn warped(plane_bytes: usize, output_bytes: usize) -> usize {
    output_bytes + 2 * plane_bytes + plane_bytes / size_of::<f32>()
}

/// The stages `AlignStackConfig::calibrate_align_stack_paths` plans with the reference found,
/// cosmic rays off: the decode raised to the statistics' copy of every channel beside the frame —
/// at these sizes more than the noise estimate holds — and the detector's planes beside each
/// decode; the warp of the demosaiced output, its source in hand; no normalization; a combine
/// holding `output_bytes`.
fn found_reference(plane_bytes: usize, decode: Footprint, output_bytes: usize) -> [Stage; 4] {
    let prepared = decode.beside(decode.kept);
    let warped = warped(plane_bytes, decode.kept);
    [
        Stage {
            kept: prepared.kept,
            held: 0,
            in_flight: prepared.transient() + DETECTOR_PLANES * plane_bytes,
            units: Units::Frames,
        },
        Stage {
            kept: warped,
            held: 0,
            in_flight: prepared.kept,
            units: Units::Frames,
        },
        normalize(warped, 0),
        combine(warped, output_bytes),
    ]
}

/// The stages of a stack decoded straight into the combine: each frame kept at `kept` with
/// `scratch` of statistics beside it; a normalization holding `normalization_bytes`; a combine
/// holding `output_bytes`.
const fn decoded_stack(
    kept: usize,
    scratch: usize,
    output_bytes: usize,
    normalization_bytes: usize,
) -> [Stage; 3] {
    [
        Stage {
            kept,
            held: 0,
            in_flight: scratch,
            units: Units::Frames,
        },
        normalize(kept, normalization_bytes),
        combine(kept, output_bytes),
    ]
}

const fn normalize(kept: usize, held: usize) -> Stage {
    Stage {
        kept,
        held,
        in_flight: 0,
        units: Units::Whole,
    }
}

/// A combine of one row, so its chunks cost nothing beside the planes it holds.
const fn combine(kept: usize, held: usize) -> Stage {
    Stage {
        kept,
        held,
        in_flight: 0,
        units: Units::Rows(1),
    }
}
