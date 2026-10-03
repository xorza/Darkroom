//! Live peak-RSS memory probes for the end-to-end stacking pipeline — the manual, at-scale
//! counterparts to the deterministic guards in `mem_budget`.
//!
//! Two probes cover the pipeline's two memory regimes on **large synthetic data**, and a third
//! runs the RAW front end on the bundled dataset:
//!
//! - [`pipeline_budget_probe`] — the **total-budget** regime. Runs the pipeline's own stages on
//!   synthetic FITS under one budget: the dark and flat masters through [`stack_cfa_master`], then
//!   the lights through [`calibrate_align_stack`] — calibrate, detect, register, warp, combine. The
//!   probe asserts peak heap stays under the budget **across every stage**, plus the masters the
//!   caller holds, proving each stage's frames free before the next loads.
//!
//! - [`align_stack_memory_probe`] — the **bounded-working-set** regime. Runs the real
//!   detect → register → warp → combine flow ([`align_and_stack`]) over a large synthetic star-field
//!   set and asserts peak heap stays within the working set the RAM path inherently needs (resident
//!   warped frames + concurrent detection scratch), with headroom — so a per-frame leak in any stage
//!   would blow the ceiling.
//!
//! - [`raw_lights_memory_probe`] (feature `real-data`) — the libraw RAW decode and demosaic that
//!   synthetic FITS skip: the dataset's lights through [`calibrate_align_stack`] with empty
//!   masters, so the peak is the decode, align and stack work alone. Under a budget the peak must
//!   stay within it; the disk tier should stay about flat in the frame count, the RAM tier linear.
//!
//! All are `#[ignore]`d: heavy and measurement-only, so run one config per process with a filter,
//! like the benches. Peak RSS is read from `/proc/self/status` (Linux-only); elsewhere the pipeline
//! still runs but the numeric assertion is skipped.
//!
//! ```sh
//! cargo test -p lumos --release pipeline_budget_probe    -- --ignored --nocapture
//! cargo test -p lumos --release align_stack_memory_probe -- --ignored --nocapture
//! cargo test -p lumos --release --features real-data raw_lights_memory_probe -- --ignored --nocapture
//! ```

use crate::math::size2us::Size2us;
use common::internals;
use std::env;
use std::fs;
use std::hint::black_box;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use common::CancelToken;
use glam::DVec2;

use crate::calibration_masters::calibration_set::CalibrationSet;
use crate::calibration_masters::master_role::MasterRole;
use crate::calibration_masters::{CalibrationMasters, DEFAULT_SIGMA_THRESHOLD, stack_cfa_master};
use crate::internals::cfa::make_cfa;
use crate::internals::mem_probe::{
    BudgetChoice, MB, RssSampler, budget_ceiling_mb, env_parse, measured, parse_budget,
    synth_frame_u16, two_x_ceiling_mb,
};
use crate::internals::synthetic::camera::Camera;
use crate::internals::synthetic::fixtures::{
    STAR_FIELD_FLUX, STAR_FIELD_FWHM, STAR_FIELD_MARGIN, STAR_FIELD_SKY, star_field,
};
use crate::internals::synthetic::observe::{Observation, render};
use crate::internals::synthetic::scene::{BackgroundField, Scene};
use crate::io::image::cfa::CfaType;
use crate::io::image::fits::cfa::save_cfa_fits;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::memory;
use crate::memory::{DETECTION_WORKING_PLANES, PerFrameBytes};
use crate::pipeline::align::align_and_stack;
use crate::pipeline::calibrate::calibrate_align_stack;
use crate::pipeline::config::{AlignStackConfig, Reference};
use crate::progress::ProgressCallback;
use crate::registration::config::Config as RegistrationConfig;
use crate::registration::resample::warp;
use crate::registration::transform::{Transform, WarpTransform};
use crate::stack_product::quality_planes::QualityPlanes;

#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn pipeline_budget_probe() -> io::Result<()> {
    let n: usize = env_parse("LUMOS_PIPE_FRAMES", 24);
    let size = Size2us::new(
        env_parse("LUMOS_PIPE_W", 6000),
        env_parse("LUMOS_PIPE_H", 6000),
    );
    let stars: usize = env_parse("LUMOS_PIPE_STARS", 2000);
    let seed: u64 = env_parse("LUMOS_PIPE_SEED", 1);
    // Default 2048 MB so the default 6000×6000 × 24 set (3.3 GB resident) overflows it → disk tier.
    let budget = parse_budget("LUMOS_PIPE_BUDGET", BudgetChoice::mb(2048));

    let base = env::var("LUMOS_PIPE_DIR").map_or_else(
        |_| internals::scratch_dir("lumos_pipeline_stack"),
        PathBuf::from,
    );
    let set_name = format!("{}x{}_n{n}_s{seed}", size.width, size.height);
    let frame_bytes = (size.pixel_count() * size_of::<f32>()) as u64;

    println!("=== lumos pipeline memory probe (total budget across stages) ===");
    println!(
        "per stage     {n} × {}×{} mono  ({:.1} MB/frame f32)",
        size.width,
        size.height,
        frame_bytes as f64 / MB as f64
    );
    println!("stages        master-dark → master-flat → lights ({stars} stars each)");
    println!("budget        {}", budget.label);
    println!();

    // One set stands in for both masters: a dark and a flat of the same pedestal subtract nothing
    // from each other, as no bias or flat-dark is given, so the flat keeps its vignetting.
    let calibration = ensure_cfa_frames(&base.join(format!("cal_{set_name}")), n, size, |i| {
        synth_frame_u16(size, i, seed)
            .into_iter()
            .map(|sample| f32::from(sample) / f32::from(u16::MAX))
            .collect()
    })?;
    let scene = Scene::random_field(
        size,
        stars,
        STAR_FIELD_FLUX,
        BackgroundField::Uniform {
            level: STAR_FIELD_SKY,
        },
        STAR_FIELD_MARGIN,
        seed,
    );
    let camera = Camera::realistic(STAR_FIELD_FWHM);
    let lights = ensure_cfa_frames(
        &base.join(format!("lights_{set_name}_{stars}")),
        n,
        size,
        |i| {
            // Deterministic dithers in ~±8 px, small enough that every light overlaps the first,
            // each with its own noise.
            let dx = ((i * 37 % 11) as f64 - 5.0) * 1.7;
            let dy = ((i * 53 % 11) as f64 - 5.0) * 1.7;
            let observation = Observation {
                transform: Transform::translation(DVec2::new(dx, dy)),
                ..Observation::reference(seed.wrapping_add(i as u64))
            };
            let frame = render(&scene, &camera, &observation);
            frame.image.channel(0).pixels().to_vec()
        },
    )?;
    println!();

    // One sampler spanning every stage: the peak it reports is the max over the whole sequence, so
    // an assertion of "peak ≤ budget" is a *total*-budget check — each stage's cache must drop
    // before the next loads, or K stages would stack to ~K× the budget.
    let sampler = RssSampler::start();
    let start = Instant::now();
    let master = |k: usize, role: MasterRole| {
        let mut config = role.stack_config();
        config.cache.memory_override = budget.memory_override;
        config.cache.cache_dir = base.join(format!("cache_{k}"));
        let stage_start = Instant::now();
        let master = stack_cfa_master(
            &calibration,
            config,
            None,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("stack a master");
        println!(
            "  [{}/3] master {role:?} ({:.2}s)",
            k + 1,
            stage_start.elapsed().as_secs_f64()
        );
        master
    };
    let dark = master(0, MasterRole::Dark);
    let flat = master(1, MasterRole::Flat);
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark,
            flat,
            bias: None,
            flat_dark: None,
        },
        DEFAULT_SIGMA_THRESHOLD,
        &CancelToken::never(),
    )
    .expect("assemble the masters");

    let mut config = AlignStackConfig::default();
    config.stack.cache.memory_override = budget.memory_override;
    config.stack.cache.cache_dir = base.join("cache_2");
    let stage_start = Instant::now();
    let result = calibrate_align_stack(
        &lights,
        &masters,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("calibrate_align_stack");
    println!(
        "  [3/3] lights ({:.2}s), {} registered, {} dropped",
        stage_start.elapsed().as_secs_f64(),
        result.alignment.registered,
        result.alignment.dropped.len()
    );
    black_box(&result);
    let total_secs = start.elapsed().as_secs_f64();

    let peak = sampler.finish();
    let anon_mb = peak.anon;
    let held_mb = masters.ram_bytes() as u64 / MB;

    println!("\n=== result ===");
    println!("time          {total_secs:.2}s over 3 stages");
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure, across ALL stages)");
    println!(
        "peak VmRSS    {} MB   (total resident, incl. mmap'd spill)",
        peak.total
    );
    println!("masters held  {held_mb} MB");

    assert_eq!(
        result.alignment.registered, n,
        "every dithered light should register (probe misconfigured?)"
    );
    // The budget stands for the memory each stage may take. The masters are outside it: the caller
    // holds them, and in a real run the system's available-memory figure already excludes them.
    // `budget_ceiling_mb` skips what no budget can hold (the `disk`/`ram` sentinels, a sub-floor
    // budget) and the off-Linux case.
    if let Some(budget_mb) = budget_ceiling_mb(anon_mb, &budget, frame_bytes) {
        let ceiling_mb = budget_mb + held_mb;
        assert!(
            anon_mb <= ceiling_mb,
            "peak heap {anon_mb} MB exceeded the {budget_mb} MB budget plus {held_mb} MB of \
             masters — a stage's memory didn't free before the next, or a stage overran its budget"
        );
        println!("budget check  OK: peak heap {anon_mb} MB ≤ {ceiling_mb} MB across all stages");
    }

    Ok(())
}

/// `n` mono-CFA FITS frames of `size` in `dir`, frame `i` holding the samples `frame(i)` gives,
/// skipping any already present so a re-run reuses the set.
fn ensure_cfa_frames(
    dir: &Path,
    n: usize,
    size: Size2us,
    frame: impl Fn(usize) -> Vec<f32>,
) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let paths: Vec<PathBuf> = (0..n)
        .map(|i| dir.join(format!("frame_{i:04}.fits")))
        .collect();
    for (i, path) in paths.iter().enumerate() {
        if path.exists() {
            continue;
        }
        let cfa = make_cfa(size, frame(i), CfaType::Mono);
        save_cfa_fits(path, &cfa)?;
        print!("\r  generating {}… {}/{n}", dir.display(), i + 1);
        io::stdout().flush().ok();
    }
    Ok(paths)
}

#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn align_stack_memory_probe() {
    let n: usize = env_parse::<usize>("LUMOS_ALIGN_FRAMES", 24).max(2);
    let size = Size2us::new(
        env_parse("LUMOS_ALIGN_W", 2000),
        env_parse("LUMOS_ALIGN_H", 2000),
    );
    let stars: usize = env_parse("LUMOS_ALIGN_STARS", 800);
    let seed: u64 = env_parse("LUMOS_ALIGN_SEED", 1);

    let frame_bytes = (size.pixel_count() * size_of::<f32>()) as u64;

    println!("=== lumos align+stack memory probe (detect → register → warp → combine) ===");
    println!(
        "frames        {n} × {}×{} ({stars} stars each, {:.1} MB/frame f32)",
        size.width,
        size.height,
        frame_bytes as f64 / MB as f64
    );

    // Build the input set: a base star field plus `n-1` small dithers of it, so registration has a
    // shared pattern to solve. Warp scratch is freed per frame; only the `n` inputs stay resident.
    let reg = RegistrationConfig::default();
    let base = star_field(size, stars, seed).image;
    let channels = base.channels();
    let gen_start = Instant::now();
    let mut frames: Vec<LinearImage> = Vec::with_capacity(n);
    frames.push(base.clone());
    for i in 1..n {
        // Deterministic dithers in ~±8 px — small enough that the shifted field still overlaps.
        let dx = ((i * 37 % 11) as f64 - 5.0) * 1.7;
        let dy = ((i * 53 % 11) as f64 - 5.0) * 1.7;
        let t = Transform::translation(DVec2::new(dx, dy));
        frames.push(warp(&base, &WarpTransform::new(t), reg.warp).image);
    }
    drop(base); // redundant with frames[0]; free it so only the `n` inputs are resident.
    println!(
        "input ready   {n} frames, {channels}-channel, {:.2} GB resident, built in {:.1}s",
        (n as u64 * channels as u64 * frame_bytes) as f64 / 1e9,
        gen_start.elapsed().as_secs_f64()
    );
    println!();

    // Sample across the whole align+stack (inputs already resident). The RAM path holds every
    // frame, so peak scales with `n` — the assertion is that it stays within the working set it
    // *needs*, not that it's flat.
    let sampler = RssSampler::start();
    let start = Instant::now();
    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let result = align_and_stack(
        frames,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("align_and_stack");
    let total_secs = start.elapsed().as_secs_f64();

    let peak = sampler.finish();
    let anon_mb = peak.anon;
    let mpix = (size.pixel_count() * n) as f64 / 1e6;

    println!("=== result ===");
    println!(
        "stacked       {}×{} × {} ch, {} registered, {} dropped",
        result.product.image.width(),
        result.product.image.height(),
        result.product.image.channels(),
        result.alignment.registered,
        result.alignment.dropped.len()
    );
    println!(
        "time          {total_secs:.2}s  ({:.0} Mpix/s over the stream)",
        mpix / total_secs.max(1e-3)
    );
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure)");
    println!("peak VmRSS    {} MB   (total resident)", peak.total);
    println!(
        "amortized     {:.2} MB heap per frame over {n} frames",
        anon_mb as f64 / n as f64
    );

    // The RAM path's working set, by the planner's own accounting: the resident warped frames
    // (pixels plus their two quality planes) and the combine's output beside them, plus
    // `threads ×` one frame's working set (the warp's source and output, or the detector's pool
    // and the statistics' copy of the frame). Peak must stay within a generous 2× of that — a
    // per-frame buffer leak in detection, warp, or the combine would push it over.
    let threads = rayon::current_num_threads();
    let dimensions = ImageDimensions::new(size, channels);
    let output_bytes = memory::frame_bytes(dimensions);
    let per_frame = PerFrameBytes::new(frame_bytes as usize, output_bytes);
    let detection = DETECTION_WORKING_PLANES * frame_bytes as usize + output_bytes;
    let resident = (n * per_frame.warped + QualityPlanes::ALL.resident_bytes(dimensions)) as u64;
    let working = (threads * per_frame.working.max(detection)) as u64;
    let ceiling_mb = two_x_ceiling_mb(resident, working);

    assert!(
        result.alignment.registered >= 2,
        "expected the dithered frames to register; only {} stacked (probe misconfigured?)",
        result.alignment.registered
    );
    if measured(anon_mb, "ceiling check") {
        assert!(
            anon_mb <= ceiling_mb,
            "peak heap {anon_mb} MB exceeded the {ceiling_mb} MB ceiling (resident warped set + \
             {threads}× detection scratch, 2× headroom) over {n} frames — a stage leaked a buffer \
             per frame, so align+stack memory scales past its working set"
        );
        println!(
            "ceiling check OK: peak heap {anon_mb} MB ≤ {ceiling_mb} MB (within the RAM path's \
             working set)"
        );
    }
}

#[cfg(feature = "real-data")]
#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn raw_lights_memory_probe() {
    use crate::internals::real_data;
    use crate::progress::StackingStage;

    let n: usize = env_parse("LUMOS_RAW_FRAMES", usize::MAX);
    let budget = parse_budget("LUMOS_RAW_BUDGET", BudgetChoice::mb(4096));
    let all = real_data::raw_frames("Lights");
    let lights = &all[..n.min(all.len())];

    println!("=== lumos RAW lights memory probe (decode → calibrate → align → stack) ===");
    println!("lights        {}", lights.len());
    println!("budget        {}", budget.label);

    let mut config = AlignStackConfig::default();
    config.registration.ransac.seed = Some(1);
    config.stack.cache.memory_override = budget.memory_override;
    // The gate opens as the preparing pass reports its last frame, so the peak splits into the
    // decode and detect pass and the register, warp and combine passes after it.
    let sampler = RssSampler::start();
    let gate = sampler.gate();
    let progress = ProgressCallback::new(move |report| {
        if report.stage != StackingStage::Preparing || report.current == report.total {
            gate.open();
        }
    });
    let start = Instant::now();
    let result = calibrate_align_stack(
        lights,
        &CalibrationMasters::default(),
        &config,
        progress,
        CancelToken::never(),
    )
    .expect("calibrate_align_stack");
    let total_secs = start.elapsed().as_secs_f64();
    let peak = sampler.finish();
    let anon_mb = peak.anon;

    let image = &result.product.image;
    let frame_bytes = (image.dimensions().sample_count() * size_of::<f32>()) as u64;
    println!(
        "stacked       {}×{} × {} ch, {} registered, {} dropped, {total_secs:.2}s",
        image.width(),
        image.height(),
        image.channels(),
        result.alignment.registered,
        result.alignment.dropped.len()
    );
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure)");
    println!(
        "  ├ prepare   {} MB   (decode, demosaic, detect)",
        peak.ungated_anon
    );
    println!(
        "  └ align     {} MB   (register, warp, combine)",
        peak.gated_anon
    );
    println!(
        "peak VmRSS    {} MB   (total resident, incl. mmap'd spill)",
        peak.total
    );
    black_box(&result);

    assert_eq!(result.alignment.registered, lights.len());
    if let Some(budget_mb) = budget_ceiling_mb(anon_mb, &budget, frame_bytes) {
        assert!(
            anon_mb <= budget_mb,
            "peak heap {anon_mb} MB exceeded the {budget_mb} MB budget on {} RAW lights",
            lights.len()
        );
        println!("budget check  OK: peak heap {anon_mb} MB ≤ {budget_mb} MB");
    }
}
