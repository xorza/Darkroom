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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use common::CancelToken;
use fits_well::{FitsWriter, Image};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

use crate::stacking::combine::config::{CombineMethod, StackConfig};
use crate::stacking::combine::progress::{ProgressCallback, StackingProgress, StackingStage};
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::combine::stack::stack;

const MB: u64 = 1024 * 1024;

/// Read a `/proc/self/status` field (`RssAnon`, `VmRSS`) in KiB. Linux-only; returns 0 elsewhere.
fn status_kb(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(field).and_then(|r| r.strip_prefix(':')) {
            return rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }
    0
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Resolved `available_memory` budget plus a human label for the report.
#[derive(Debug)]
struct BudgetChoice {
    available_memory: Option<u64>,
    label: String,
}

fn parse_budget() -> BudgetChoice {
    match std::env::var("LUMOS_BUDGET").ok().as_deref() {
        None | Some("") | Some("auto") => BudgetChoice {
            available_memory: None,
            label: "auto (system available)".into(),
        },
        // u64::MAX ⇒ everything fits ⇒ in-memory tier. 1 byte ⇒ nothing fits ⇒ spill tier.
        Some("ram") => BudgetChoice {
            available_memory: Some(u64::MAX),
            label: "ram (force resident)".into(),
        },
        Some("disk") => BudgetChoice {
            available_memory: Some(1),
            label: "disk (force spill)".into(),
        },
        Some(n) => {
            let mb: u64 = n
                .parse()
                .unwrap_or_else(|_| panic!("LUMOS_BUDGET: expected ram|disk|auto|<MB>, got {n:?}"));
            BudgetChoice {
                available_memory: Some(mb * MB),
                label: format!("{mb} MB"),
            }
        }
    }
}

/// One synthetic 16-bit mono frame: a vignetted background pedestal + Gaussian read noise, plus a
/// handful of bright per-frame outliers (cosmic-ray stand-ins). The outliers differ every frame, so
/// sigma-clipping and median combine actually have something to reject — a mean combine would keep
/// them. Deterministic in `(seed, frame_idx)`; rows are generated in parallel with per-row RNGs.
fn synth_frame_u16(width: usize, height: usize, frame_idx: usize, seed: u64) -> Vec<u16> {
    const PEDESTAL: f32 = 0.08;
    const READ_NOISE: f32 = 0.012;

    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_r2 = cx * cx + cy * cy;

    let mut data = vec![0u16; width * height];
    let frame_mix = seed ^ (frame_idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);

    data.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let mut rng =
            ChaCha8Rng::seed_from_u64(frame_mix ^ (y as u64).wrapping_mul(0x0000_0100_0000_0001));
        let dy = y as f32 - cy;
        for (x, px) in row.iter_mut().enumerate() {
            let dx = x as f32 - cx;
            // Radial vignette in ~[0.75, 1.0] and a mild diagonal gradient give the field structure.
            let vignette = 1.0 - 0.25 * (dx * dx + dy * dy) / max_r2;
            let gradient = 0.03 * (x as f32 / width as f32 + y as f32 / height as f32);
            // Irwin–Hall (four uniforms) ≈ zero-mean Gaussian read noise.
            let g: f32 = rng.random::<f32>()
                + rng.random::<f32>()
                + rng.random::<f32>()
                + rng.random::<f32>()
                - 2.0;
            let v = PEDESTAL * vignette + gradient + READ_NOISE * g;
            *px = (v.clamp(0.0, 1.0) * 65535.0) as u16;
        }
    });

    let n_outliers = (width * height / 50_000).max(4);
    let mut rng = ChaCha8Rng::seed_from_u64(frame_mix.rotate_left(17));
    for _ in 0..n_outliers {
        let x = rng.random_range(0..width);
        let y = rng.random_range(0..height);
        let level = rng.random_range(0.85f32..1.0);
        data[y * width + x] = (level * 65535.0) as u16;
    }
    data
}

/// Write `data` as a 16-bit (BITPIX=16, BZERO=32768) FITS image, reusing `buf` as the encode
/// scratch so a whole stack allocates it once. `std::fs::write` flushes reliably (unlike a
/// `BufWriter` dropped inside the FITS writer).
fn write_fits_u16(
    path: &Path,
    width: usize,
    height: usize,
    data: &[u16],
    buf: &mut Vec<u8>,
) -> io::Result<()> {
    let image = Image::from_u16(vec![width, height], data);
    buf.clear();
    FitsWriter::new(&mut *buf)
        .write_image(&image)
        .map_err(io::Error::other)?;
    std::fs::write(path, &buf)
}

/// The generated frame set: paths plus what it cost to materialize the missing ones.
#[derive(Debug)]
struct FrameSet {
    paths: Vec<PathBuf>,
    generated: usize,
    gen_secs: f64,
    bytes_on_disk: u64,
}

/// Generate the frame set into `dir`, skipping frames already present.
fn ensure_frames(
    dir: &Path,
    n: usize,
    width: usize,
    height: usize,
    seed: u64,
) -> io::Result<FrameSet> {
    std::fs::create_dir_all(dir)?;
    let paths: Vec<PathBuf> = (0..n)
        .map(|i| dir.join(format!("frame_{i:04}.fits")))
        .collect();

    let start = Instant::now();
    let mut buf = Vec::new();
    let mut generated = 0;
    for (i, path) in paths.iter().enumerate() {
        if path.exists() {
            continue;
        }
        let data = synth_frame_u16(width, height, i, seed);
        write_fits_u16(path, width, height, &data, &mut buf)?;
        generated += 1;
        print!("\r  generating frames… {}/{}", i + 1, n);
        io::stdout().flush().ok();
    }
    if generated > 0 {
        println!();
    }
    let bytes_on_disk = paths
        .iter()
        .filter_map(|p| p.metadata().ok())
        .map(|m| m.len())
        .sum();

    Ok(FrameSet {
        paths,
        generated,
        gen_secs: start.elapsed().as_secs_f64(),
        bytes_on_disk,
    })
}

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
    let budget = parse_budget();

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

    let frames = ensure_frames(&frames_dir, n, width, height, seed)?;
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
    // must bound) vs the *combine* steady state (resident frames + chunk buffers). `combining` flips
    // when the first Processing progress arrives, so each sample lands in the right bucket.
    let stop = Arc::new(AtomicBool::new(false));
    let combining = Arc::new(AtomicBool::new(false));
    let peak_anon = Arc::new(AtomicU64::new(0));
    let peak_total = Arc::new(AtomicU64::new(0));
    let peak_load_anon = Arc::new(AtomicU64::new(0));
    let peak_combine_anon = Arc::new(AtomicU64::new(0));
    let sampler = {
        let (stop, combining) = (stop.clone(), combining.clone());
        let (peak_anon, peak_total) = (peak_anon.clone(), peak_total.clone());
        let (peak_load_anon, peak_combine_anon) =
            (peak_load_anon.clone(), peak_combine_anon.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let anon = status_kb("RssAnon");
                peak_anon.fetch_max(anon, Ordering::Relaxed);
                peak_total.fetch_max(status_kb("VmRSS"), Ordering::Relaxed);
                if combining.load(Ordering::Relaxed) {
                    peak_combine_anon.fetch_max(anon, Ordering::Relaxed);
                } else {
                    peak_load_anon.fetch_max(anon, Ordering::Relaxed);
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        })
    };

    // Split load vs combine wall-clock via the progress stages, and print a live line.
    let start = Instant::now();
    let loaded = Arc::new(AtomicUsize::new(0));
    let combine_start_us = Arc::new(AtomicU64::new(0));
    let progress: ProgressCallback = {
        let (loaded, combine_start_us) = (loaded.clone(), combine_start_us.clone());
        let combining = combining.clone();
        ProgressCallback::new(Arc::new(move |p: StackingProgress| match p.stage {
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
                combining.store(true, Ordering::Relaxed);
                combine_start_us
                    .compare_exchange(
                        0,
                        start.elapsed().as_micros() as u64,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .ok();
            }
        }))
    };

    let result =
        stack(&frames.paths, config, progress, CancelToken::never()).expect("stack failed");
    let total_secs = start.elapsed().as_secs_f64();

    stop.store(true, Ordering::Relaxed);
    sampler.join().ok();

    let combine_secs = total_secs - combine_start_us.load(Ordering::Relaxed) as f64 / 1e6;
    let load_secs = total_secs - combine_secs;
    let dims = result.image.dimensions();
    let mpix = (width * height * n) as f64 / 1e6;
    let anon_mb = peak_anon.load(Ordering::Relaxed) / 1024;
    let total_mb = peak_total.load(Ordering::Relaxed) / 1024;
    let load_anon_mb = peak_load_anon.load(Ordering::Relaxed) / 1024;
    let combine_anon_mb = peak_combine_anon.load(Ordering::Relaxed) / 1024;

    println!("\n");
    println!("=== result ===");
    println!(
        "master        {}×{} × {} ch, mean {:.4}",
        dims.size.x,
        dims.size.y,
        dims.channels,
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
    // stay within it. Skip the checks that can't hold: the `disk`/`ram` sentinels aren't real
    // budgets, and below one decode's transient (~2× a frame) the engine can't honor any budget —
    // it must hold at least one in-flight decode plus the output frame.
    if let Some(budget_bytes) = budget.available_memory {
        let floor = 3 * bytes_per_frame; // one in-flight decode (~2× frame) + the output frame
        if budget_bytes != u64::MAX && budget_bytes >= floor {
            let budget_mb = budget_bytes / MB;
            assert!(
                anon_mb <= budget_mb,
                "peak heap {anon_mb} MB exceeded the {budget_mb} MB budget \
                 (load burst {load_anon_mb} MB, combine {combine_anon_mb} MB) — \
                 memory-tier accounting regressed"
            );
            println!("budget check  OK: peak heap {anon_mb} MB ≤ {budget_mb} MB budget");
        }
    }

    std::hint::black_box(&result);
    Ok(())
}
