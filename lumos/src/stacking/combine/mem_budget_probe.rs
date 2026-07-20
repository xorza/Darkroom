//! Live peak-RSS memory probe for the from-paths stacker — the manual, at-scale counterpart to the
//! deterministic guards in `mem_budget_tests`.
//!
//! Builds a stack of synthetic 16-bit FITS frames on disk, then combines them into one master
//! through [`stack`] — the same memory-tiered `CacheCore` engine calibration-master creation
//! (`stack_cfa_master`) drives. It *watches* how the pipeline behaves when the frames don't all fit
//! in RAM: with a small budget the combiner spills each decoded frame to disk and mmaps it back
//! (peak heap ≈ one chunk, flat in the frame count); with a large budget every frame stays resident
//! (peak heap ≈ linear in the frame count). For an explicit numeric budget it also asserts peak heap
//! stays within it — the live check the projection test in `mem_budget_tests` can only model.
//!
//! `#[ignore]`d because it's heavy and measurement-only: peak RSS is a per-process high-water mark,
//! so run **one config per process** with a filter, exactly like the benches:
//! ```sh
//! cargo test -p lumos --release master_stack_memory_probe -- --ignored --nocapture
//! ```
//! Sweep the budget (frames are cached under `LUMOS_DIR` and reused) to see the disk tier stay flat
//! while the RAM tier grows:
//! ```sh
//! for b in disk 1024 2048 4096 ram; do
//!   LUMOS_BUDGET=$b cargo test -p lumos --release master_stack_memory_probe -- --ignored --nocapture
//! done
//! ```
//!
//! Self-contained: needs no real data and no libraw.
//!
//! ```text
//! LUMOS_FRAMES   frame count            (default 24)
//! LUMOS_W        frame width  in px     (default 6000)
//! LUMOS_H        frame height in px     (default 6000)
//! LUMOS_BUDGET   memory budget: a number in MB, or `ram` (force resident),
//!                `disk` (force spill), or `auto` (query the system)  (default auto)
//! LUMOS_METHOD   combine: `sigma` | `median` | `mean`               (default sigma)
//! LUMOS_SEED     RNG seed                                           (default 1)
//! LUMOS_DIR      base dir for frames + cache   (default <repo>/.tmp/lumos_master_stack)
//! LUMOS_KEEP     keep the on-disk stacking cache after the run (`1`) (default 0)
//! ```
//!
//! Caveat: `LUMOS_DIR` must be on a **real disk**, not tmpfs. If the spill cache lands on tmpfs
//! (common for `/tmp`), the "disk" tier's mmap pages live in RAM and the measurement is a lie. The
//! default (`<repo>/.tmp`) is disk-backed.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use common::CancelToken;

use crate::stacking::combine::config::{CombineMethod, StackConfig};
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::combine::stack::stack;
use crate::stacking::progress::{ProgressCallback, StackingProgress, StackingStage};
use crate::testing::mem_probe::{
    BudgetChoice, MB, RssSampler, budget_ceiling_mb, ensure_frames, env_parse, parse_budget,
};

fn build_config(
    method: &str,
    available_memory: Option<u64>,
    cache_dir: PathBuf,
    keep: bool,
) -> StackConfig {
    let mut config = match method {
        "median" => StackConfig::median(),
        "mean" => StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            ..StackConfig::sigma_clipped(3.0)
        },
        "sigma" => StackConfig::sigma_clipped(3.0),
        other => panic!("LUMOS_METHOD: expected sigma|median|mean, got {other:?}"),
    };
    config.cache.available_memory = available_memory;
    config.cache.cache_dir = cache_dir;
    config.cache.keep_cache = keep;
    config
}

#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn master_stack_memory_probe() -> io::Result<()> {
    let n: usize = env_parse("LUMOS_FRAMES", 24);
    let width: usize = env_parse("LUMOS_W", 6000);
    let height: usize = env_parse("LUMOS_H", 6000);
    let seed: u64 = env_parse("LUMOS_SEED", 1);
    let method = std::env::var("LUMOS_METHOD").unwrap_or_else(|_| "sigma".into());
    let keep = env_parse("LUMOS_KEEP", 0) != 0;
    let budget = parse_budget("LUMOS_BUDGET", BudgetChoice::auto());

    let base = std::env::var("LUMOS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.tmp/lumos_master_stack")
        });
    let frames_dir = base.join(format!("{width}x{height}_n{n}_s{seed}"));
    let cache_dir = base.join("cache");

    let bytes_per_frame = (width * height * 4) as u64;
    let resident_if_ram = bytes_per_frame * n as u64;

    let mut sys = sysinfo::System::new();
    sys.refresh_memory();

    println!("=== lumos master-stack memory probe ===");
    println!(
        "frames        {n} × {width}×{height} mono  ({:.1} MB/frame f32)",
        bytes_per_frame as f64 / MB as f64
    );
    println!("method        {method}");
    println!("budget        {}", budget.label);
    println!(
        "system RAM    {:.1} GB total, {:.1} GB available",
        sys.total_memory() as f64 / 1e9,
        sys.available_memory() as f64 / 1e9
    );
    println!(
        "resident set  {:.2} GB if fully in-memory (Σ frames as f32)",
        resident_if_ram as f64 / 1e9
    );
    if let Some(avail) = budget.available_memory {
        // Mirror of the internal tier rule: usable = 75% of the budget; spill if the set exceeds it.
        let usable = (avail as u128 * 75 / 100) as u64;
        let tier = if resident_if_ram <= usable {
            "in-memory (resident)"
        } else {
            "disk (spill + mmap)"
        };
        println!("predicted     {tier}");
    }
    println!("frames dir    {}", frames_dir.display());
    println!();

    let frames = ensure_frames(&frames_dir, "frame", n, width, height, seed)?;
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

    let config = build_config(&method, budget.available_memory, cache_dir, keep);

    // Sample peak heap (RssAnon — the OOM-relevant, non-reclaimable metric) and total resident
    // (VmRSS, which includes reclaimable mmap'd spill pages) for the duration of the stack only.
    // Peak heap is split by phase: the *load* burst (concurrent decode transients — what the budget
    // must bound, the gate-closed / ungated peak) vs the *combine* steady state (resident frames +
    // chunk buffers, the gate-open peak). The gate opens when the first Processing progress arrives.
    let sampler = RssSampler::start();
    let combining = sampler.gate();

    // Split load vs combine wall-clock via the progress stages, and print a live line.
    let start = Instant::now();
    let loaded = Arc::new(AtomicUsize::new(0));
    let combine_start_us = Arc::new(AtomicU64::new(0));
    let progress: ProgressCallback = {
        let (loaded, combine_start_us) = (loaded.clone(), combine_start_us.clone());
        let combining = combining.clone();
        ProgressCallback::new(move |p: StackingProgress| match p.stage {
            StackingStage::Loading => {
                let done = loaded.fetch_add(1, Ordering::Relaxed) + 1;
                let secs = start.elapsed().as_secs_f64();
                print!(
                    "\r  loading {done}/{} ({:.1} frames/s)   ",
                    p.total,
                    done as f64 / secs.max(1e-3)
                );
                io::stdout().flush().ok();
            }
            StackingStage::Processing => {
                combining.open();
                combine_start_us
                    .compare_exchange(
                        0,
                        start.elapsed().as_micros() as u64,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .ok();
            }
        })
    };

    let result =
        stack(&frames.paths, config, progress, CancelToken::never()).expect("stack failed");
    let total_secs = start.elapsed().as_secs_f64();

    let peak = sampler.finish();
    let combine_secs = total_secs - combine_start_us.load(Ordering::Relaxed) as f64 / 1e6;
    let load_secs = total_secs - combine_secs;
    let dims = result.image.dimensions();
    let mpix = (width * height * n) as f64 / 1e6;
    let anon_mb = peak.anon_mb;
    let total_mb = peak.total_mb;
    let load_anon_mb = peak.ungated_anon_mb;
    let combine_anon_mb = peak.gated_anon_mb;

    println!("\n");
    println!("=== result ===");
    println!(
        "master        {}×{} × {} ch, mean {:.4}",
        dims.width(),
        dims.height(),
        dims.channels(),
        result.image.mean()
    );
    println!(
        "time          {total_secs:.2}s total  ({load_secs:.2}s load + {combine_secs:.2}s combine)"
    );
    println!(
        "throughput    {:.0} Mpix/s over the stack",
        mpix / total_secs.max(1e-3)
    );
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure)");
    println!("  ├ load      {load_anon_mb} MB   (concurrent decode-transient burst)");
    println!("  └ combine   {combine_anon_mb} MB   (resident frames + chunk buffers)");
    println!("peak VmRSS    {total_mb} MB   (total resident, incl. mmap'd spill)");
    println!(
        "observed tier {}",
        if (anon_mb as f64 * 1.4) < (resident_if_ram as f64 / MB as f64) {
            "disk (heap ≪ resident set ⇒ frames were spilled)"
        } else {
            "in-memory (heap ≈ resident set ⇒ frames stayed in RAM)"
        }
    );

    // The budget is a hard heap ceiling: with the decode-transient accounting fixed, peak heap must
    // stay within it. `budget_ceiling_mb` skips the cases no budget can hold (the `disk`/`ram`
    // sentinels, a sub-floor budget) and the off-Linux no-measurement case.
    if let Some(budget_mb) = budget_ceiling_mb(anon_mb, &budget, bytes_per_frame) {
        assert!(
            anon_mb <= budget_mb,
            "peak heap {anon_mb} MB exceeded the {budget_mb} MB budget \
             (load burst {load_anon_mb} MB, combine {combine_anon_mb} MB) — \
             memory-tier accounting regressed"
        );
        println!("budget check  OK: peak heap {anon_mb} MB ≤ {budget_mb} MB budget");
    }

    std::hint::black_box(&result);
    Ok(())
}
