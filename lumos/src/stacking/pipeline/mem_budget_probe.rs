//! Live peak-RSS memory probes for the end-to-end stacking pipeline — the manual, at-scale
//! counterparts to the deterministic guards in `mem_budget_tests`.
//!
//! Two probes, covering the pipeline's two memory regimes on **large synthetic data**:
//!
//! - [`pipeline_stack_budget_probe`] — the **total-budget** regime. Stacks a large synthetic FITS set
//!   through the real memory-tiered engine ([`stack`]) repeatedly, once per stage the pipeline runs
//!   (master dark/flat/bias + the final light combine). One budget is set for the whole run; the
//!   probe asserts peak heap stays under it **across every stage**, proving each stage's frames free
//!   before the next loads — so the pipeline respects one total budget regardless of stage count.
//!   This is the same `CacheCore` tiering that `CalibrationMasters::from_files` and the final combine
//!   both drive; the file loaders they use are libraw-RAW-only, so this exercises the shared engine
//!   through the FITS-capable `stack()` instead.
//!
//! - [`align_stack_memory_probe`] — the **bounded-working-set** regime. Runs the real
//!   detect → register → warp → combine flow ([`align_and_stack`]) over a large synthetic star-field
//!   set and asserts peak heap stays within the working set the RAM path inherently needs (resident
//!   warped frames + concurrent detection scratch), with headroom — so a per-frame leak in any stage
//!   would blow the ceiling.
//!
//! Both are `#[ignore]`d: heavy and measurement-only, so run one config per process with a filter,
//! like the benches. Peak RSS is read from `/proc/self/status` (Linux-only); elsewhere the pipeline
//! still runs but the numeric assertion is skipped.
//!
//! ```sh
//! cargo test -p lumos --release pipeline_stack_budget_probe -- --ignored --nocapture
//! cargo test -p lumos --release align_stack_memory_probe    -- --ignored --nocapture
//! ```
//!
//! What neither covers: the libraw RAW decode + demosaic arena, and the RAW file-based
//! `from_files` / `calibrate_align_stack` orchestration — those need real RAW data (the real-data
//! tests are the hook). Everything downstream of the decode is exercised here on synthetic data.

use std::hint::black_box;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use common::CancelToken;
use glam::DVec2;

use crate::AstroImage;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::stack::stack;
use crate::stacking::pipeline::align::align_and_stack;
use crate::stacking::pipeline::config::{AlignStackConfig, Reference};
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::stacking::registration::warp;
use crate::testing::mem_probe::{
    BudgetChoice, MB, RssSampler, budget_ceiling_mb, ensure_frames, env_parse, measured,
    parse_budget, two_x_ceiling_mb,
};
use crate::testing::synthetic::fixtures::star_field;

#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn pipeline_stack_budget_probe() -> io::Result<()> {
    let n: usize = env_parse("LUMOS_PIPE_FRAMES", 24);
    let width: usize = env_parse("LUMOS_PIPE_W", 6000);
    let height: usize = env_parse("LUMOS_PIPE_H", 6000);
    let seed: u64 = env_parse("LUMOS_PIPE_SEED", 1);
    // Default 2048 MB so the default 6000×6000 × 24 set (3.3 GB resident) overflows it → disk tier.
    let budget = parse_budget("LUMOS_PIPE_BUDGET", BudgetChoice::mb(2048));

    let base = std::env::var("LUMOS_PIPE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.tmp/lumos_pipeline_stack")
        });
    let frames_dir = base.join(format!("{width}x{height}_n{n}_s{seed}"));

    let frame_bytes = (width * height * std::mem::size_of::<f32>()) as u64;
    let resident_if_ram = frame_bytes * n as u64;

    // The pipeline's stacking sequence: three calibration masters + the final light combine. The same
    // synthetic set stands in for each — memory behavior is identical across roles (same engine, same
    // frame size), and reusing it keeps the on-disk fixture to one set instead of four.
    let stages = ["master-dark", "master-flat", "master-bias", "light-combine"];

    println!("=== lumos pipeline stacking memory probe (total budget across stages) ===");
    println!(
        "per stage     {n} × {width}×{height} mono  ({:.1} MB/frame f32)",
        frame_bytes as f64 / MB as f64
    );
    println!("stages        {} ({})", stages.len(), stages.join(" → "));
    println!("budget        {}", budget.label);
    println!(
        "resident set  {:.2} GB per stage if fully in-memory (Σ frames as f32)",
        resident_if_ram as f64 / 1e9
    );
    if let Some(avail) = budget.available_memory {
        let usable = (avail as u128 * 75 / 100) as u64;
        let tier = if resident_if_ram <= usable {
            "in-memory (resident)"
        } else {
            "disk (spill + mmap)"
        };
        println!("predicted     {tier} per stage");
    }
    println!();

    let frames = ensure_frames(&frames_dir, "pipe", n, width, height, seed)?;
    println!(
        "frames ready  {} on disk ({:.2} GB){}",
        n,
        frames.bytes_on_disk as f64 / 1e9,
        if frames.generated > 0 {
            format!(
                ", generated {} in {:.1}s",
                frames.generated, frames.gen_secs
            )
        } else {
            " (all reused)".into()
        }
    );
    println!();

    // One sampler spanning every stage: the peak it reports is the max over the whole sequence, so an
    // assertion of "peak ≤ budget" is a *total*-budget check — each stage's cache must drop before the
    // next loads, or K stages would stack to ~K× the budget.
    let sampler = RssSampler::start();
    let start = Instant::now();
    for (k, stage) in stages.iter().enumerate() {
        let mut config = StackConfig::sigma_clipped(3.0);
        config.cache.available_memory = budget.available_memory;
        // A per-stage cache dir under the (real-disk) base, removed on drop so temp disk doesn't grow
        // across stages either. Distinct dirs avoid any cross-stage file reuse confusing the tiering.
        config.cache.cache_dir = base.join(format!("cache_{k}"));
        config.cache.keep_cache = false;

        let stage_start = Instant::now();
        let result = stack(
            &frames.paths,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("stack failed");
        println!(
            "  [{}/{}] {stage:<14} mean {:.4}  ({:.2}s)",
            k + 1,
            stages.len(),
            result.image.mean(),
            stage_start.elapsed().as_secs_f64()
        );
        black_box(&result);
    }
    let total_secs = start.elapsed().as_secs_f64();

    let peak = sampler.finish();
    let anon_mb = peak.anon_mb;
    let total_stacks = stages.len();
    let mpix = (width * height * n * total_stacks) as f64 / 1e6;

    println!("\n=== result ===");
    println!(
        "time          {total_secs:.2}s over {total_stacks} stages ({:.0} Mpix/s)",
        mpix / total_secs.max(1e-3)
    );
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure, across ALL stages)");
    println!(
        "peak VmRSS    {} MB   (total resident, incl. mmap'd spill)",
        peak.total_mb
    );

    // The budget is a hard heap ceiling for the whole sequence. `budget_ceiling_mb` skips what no
    // budget can hold (the `disk`/`ram` sentinels, a sub-floor budget) and the off-Linux case.
    if let Some(budget_mb) = budget_ceiling_mb(anon_mb, &budget, frame_bytes) {
        assert!(
            anon_mb <= budget_mb,
            "peak heap {anon_mb} MB exceeded the {budget_mb} MB budget across {total_stacks} \
             stages — a stage's memory didn't free before the next, so the pipeline's total \
             footprint scales with the stage count instead of respecting one budget"
        );
        println!(
            "budget check  OK: peak heap {anon_mb} MB ≤ {budget_mb} MB across all {total_stacks} \
             stages (memory freed between stages)"
        );
    }

    Ok(())
}

/// Generous per-frame working set, in f32 planes, for a concurrent detector: the star-detection pool
/// (~6 image planes) plus slack for its transient (non-pooled) allocations. The align probe's peak is
/// bounded by `threads ×` this (concurrent detection) plus the resident warped set.
const DETECT_WORKING_PLANES: usize = 8;

#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn align_stack_memory_probe() {
    let n: usize = env_parse::<usize>("LUMOS_ALIGN_FRAMES", 24).max(2);
    let width: usize = env_parse("LUMOS_ALIGN_W", 2000);
    let height: usize = env_parse("LUMOS_ALIGN_H", 2000);
    let stars: usize = env_parse("LUMOS_ALIGN_STARS", 800);
    let seed: u64 = env_parse("LUMOS_ALIGN_SEED", 1);

    let frame_bytes = (width * height * std::mem::size_of::<f32>()) as u64;

    println!("=== lumos align+stack memory probe (detect → register → warp → combine) ===");
    println!(
        "frames        {n} × {width}×{height} ({stars} stars each, {:.1} MB/frame f32)",
        frame_bytes as f64 / MB as f64
    );

    // Build the input set: a base star field plus `n-1` small dithers of it, so registration has a
    // shared pattern to solve. Warp scratch is freed per frame; only the `n` inputs stay resident.
    let reg = RegistrationConfig::default();
    let base = star_field(width, height, stars, seed).image;
    let channels = base.channels();
    let gen_start = Instant::now();
    let mut frames: Vec<AstroImage> = Vec::with_capacity(n);
    frames.push(base.clone());
    for i in 1..n {
        // Deterministic dithers in ~±8 px — small enough that the shifted field still overlaps.
        let dx = ((i * 37 % 11) as f64 - 5.0) * 1.7;
        let dy = ((i * 53 % 11) as f64 - 5.0) * 1.7;
        let t = Transform::translation(DVec2::new(dx, dy));
        frames.push(warp(&base, &WarpTransform::new(t), &reg.warp).image);
    }
    drop(base); // redundant with frames[0]; free it so only the `n` inputs are resident.
    println!(
        "input ready   {n} frames, {channels}-channel, {:.2} GB resident, built in {:.1}s",
        (n as u64 * channels as u64 * frame_bytes) as f64 / 1e9,
        gen_start.elapsed().as_secs_f64()
    );
    println!();

    // Sample across the whole align+stack (inputs already resident). The RAM path holds every frame,
    // so peak scales with `n` — the assertion is that it stays within the working set it *needs*, not
    // that it's flat.
    let sampler = RssSampler::start();
    let start = Instant::now();
    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let result = align_and_stack(frames, &config, CancelToken::never()).expect("align_and_stack");
    let total_secs = start.elapsed().as_secs_f64();

    let peak = sampler.finish();
    let anon_mb = peak.anon_mb;
    let mpix = (width * height * n) as f64 / 1e6;

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
    println!("peak VmRSS    {} MB   (total resident)", peak.total_mb);
    println!(
        "amortized     {:.2} MB heap per frame over {n} frames",
        anon_mb as f64 / n as f64
    );

    // The RAM path's working set: the resident warped frames (channels + one coverage plane each)
    // that `stack_images` holds, plus concurrent detection scratch (`threads ×` the per-detector
    // pool). Peak must stay within a generous 2× of that — a per-frame buffer leak in detection,
    // warp, or the combine would push it over.
    let threads = rayon::current_num_threads();
    let resident_planes = (n * (channels + 1)) as u64;
    let working_planes = (DETECT_WORKING_PLANES * threads) as u64;
    let ceiling_mb = two_x_ceiling_mb(resident_planes * frame_bytes, working_planes * frame_bytes);

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
