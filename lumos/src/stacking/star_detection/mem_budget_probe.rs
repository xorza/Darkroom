//! Live peak-RSS memory probe for the star-detection pipeline — the manual, at-scale counterpart to
//! the deterministic guard in `mem_budget_tests`.
//!
//! Detects stars across a stream of synthetic frames through one reused [`StarDetector`] and
//! *watches* peak heap. The point it demonstrates: because the detector recycles its image-sized
//! scratch through a [`DetectionResources`], peak heap is bounded by *one* detection's working set plus the
//! few frames kept resident — **flat in the frame count**, not linear. It holds only a small ring of
//! frames in RAM and detects them round-robin, so if a stage leaked a buffer per frame, peak heap
//! would climb with the frame count and blow the ceiling this test asserts.
//!
//! `#[ignore]`d because it's heavy and measurement-only: peak RSS is a per-process high-water mark,
//! so run **one config per process** with a filter, exactly like the benches:
//! ```sh
//! cargo test -p lumos --release detect_memory_probe -- --ignored --nocapture
//! ```
//! Sweep the frame count to see peak heap stay flat while per-frame amortized cost falls:
//! ```sh
//! for n in 8 32 128 512; do
//!   LUMOS_SD_FRAMES=$n cargo test -p lumos --release detect_memory_probe -- --ignored --nocapture
//! done
//! ```
//!
//! Self-contained: renders its own synthetic star fields, needs no real data and no libraw.
//!
//! ```text
//! LUMOS_SD_FRAMES   detections to run             (default 64)
//! LUMOS_SD_W        frame width  in px            (default 3000)
//! LUMOS_SD_H        frame height in px            (default 3000)
//! LUMOS_SD_STARS    stars per synthetic frame     (default 1500)
//! LUMOS_SD_RING     distinct frames kept resident (default 4)
//! LUMOS_SD_SEED     RNG seed                       (default 1)
//! LUMOS_SD_PRESET   detector preset: default | wide | high_res | crowded | precise  (default default)
//! LUMOS_SD_REUSE    1 = one reused detector (pool recycles); 0 = a fresh detector per frame  (default 1)
//! ```
//!
//! Heap is read from `/proc/self/status` (`RssAnon`), so the numeric ceiling is only enforced on
//! Linux; elsewhere the run still exercises the pipeline but the measurement is reported as
//! unavailable and the assertion is skipped.

use std::io::{self, Write};
use std::time::Instant;

use crate::io::astro_image::AstroImage;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::mem_probe::{MB, RssSampler, env_parse, measured, two_x_ceiling_mb};
use crate::testing::synthetic::fixtures::star_field;

/// Generous upper bound on the detector's per-detection working set, in image-sized f32 planes:
/// the pooled f32 scratch, plus the bitmasks and label map counted as their f32-equivalent, plus
/// slack for transient (non-pooled) allocations. Used only to size the peak-heap ceiling; the exact
/// pool footprint is pinned in `mem_budget_tests`.
const WORKING_SET_PLANES: u64 = 12;

fn preset_config() -> Config {
    match std::env::var("LUMOS_SD_PRESET").ok().as_deref() {
        None | Some("") | Some("default") => Config::default(),
        Some("wide") => Config::wide_field(),
        Some("high_res") => Config::high_resolution(),
        Some("crowded") => Config::crowded_field(),
        Some("precise") => Config::precise_ground(),
        Some(other) => {
            panic!("LUMOS_SD_PRESET: expected default|wide|high_res|crowded|precise, got {other:?}")
        }
    }
}

#[test]
#[ignore = "manual live peak-RSS probe; run explicitly with a filter, one config per process"]
fn detect_memory_probe() {
    let n: usize = env_parse("LUMOS_SD_FRAMES", 64);
    let width: usize = env_parse("LUMOS_SD_W", 3000);
    let height: usize = env_parse("LUMOS_SD_H", 3000);
    let stars: usize = env_parse("LUMOS_SD_STARS", 1500);
    let ring: usize = env_parse::<usize>("LUMOS_SD_RING", 4).max(1).min(n.max(1));
    let seed: u64 = env_parse("LUMOS_SD_SEED", 1);
    let reuse = env_parse("LUMOS_SD_REUSE", 1) != 0;
    let config = preset_config();

    let plane_bytes = (width * height * std::mem::size_of::<f32>()) as u64;

    println!("=== lumos star-detection memory probe ===");
    println!(
        "detections    {n} over a ring of {ring} × {width}×{height} synthetic frames ({stars} stars each)"
    );
    println!(
        "preset        {}",
        std::env::var("LUMOS_SD_PRESET").unwrap_or_else(|_| "default".into())
    );
    println!(
        "detector      {}",
        if reuse {
            "reused (pool recycles across frames)"
        } else {
            "fresh per frame (pool discarded each detect)"
        }
    );
    println!(
        "plane size    {:.1} MB/f32 plane",
        plane_bytes as f64 / MB as f64
    );

    // Render the resident ring once. These are the only frames held in RAM for the whole run, so the
    // resident-set contribution to peak heap is `ring` frames — independent of `n`.
    let gen_start = Instant::now();
    let frames: Vec<AstroImage> = (0..ring)
        .map(|i| star_field(width, height, stars, seed ^ i as u64).image)
        .collect();
    let channels = frames[0].channels() as u64;
    let resident_bytes = ring as u64 * channels * plane_bytes;
    println!(
        "ring ready    {ring} frames, {channels}-channel, {:.2} GB resident, rendered in {:.1}s",
        resident_bytes as f64 / 1e9,
        gen_start.elapsed().as_secs_f64()
    );
    println!();

    // Sample peak heap (RssAnon — the OOM-relevant figure) and total resident (VmRSS) for the
    // detection loop. The gate opens after the first detection — the one that allocates the pool —
    // so the gated peak is the steady state; it must not climb with the frame count (the flat-in-n
    // proof).
    let sampler = RssSampler::start();
    let steady_gate = sampler.gate();

    let start = Instant::now();
    let mut reused = StarDetector::from_config(config.clone()).unwrap();
    let mut total_stars = 0usize;
    for i in 0..n {
        let frame = &frames[i % ring];
        let result = if reuse {
            reused.detect(frame)
        } else {
            StarDetector::from_config(config.clone())
                .unwrap()
                .detect(frame)
        };
        total_stars += result.stars.len();
        if i == 0 {
            steady_gate.open();
        }
        let secs = start.elapsed().as_secs_f64();
        print!(
            "\r  detecting {}/{n} ({:.1} frames/s)   ",
            i + 1,
            (i + 1) as f64 / secs.max(1e-3)
        );
        io::stdout().flush().ok();
    }
    let total_secs = start.elapsed().as_secs_f64();

    let peak = sampler.finish();
    let anon_mb = peak.anon_mb;
    let steady_mb = peak.gated_anon_mb;
    let total_mb = peak.total_mb;
    let mpix = (width * height * n) as f64 / 1e6;

    println!("\n");
    println!("=== result ===");
    println!(
        "detected      {total_stars} stars total ({:.0} avg/frame)",
        total_stars as f64 / n as f64
    );
    println!(
        "time          {total_secs:.2}s  ({:.0} Mpix/s over the stream)",
        mpix / total_secs.max(1e-3)
    );
    println!("peak RssAnon  {anon_mb} MB   (heap — the OOM-relevant figure)");
    println!("  └ steady    {steady_mb} MB   (after the pool-allocating first detection)");
    println!("peak VmRSS    {total_mb} MB   (total resident)");
    println!(
        "amortized     {:.2} MB heap per detection over {n} frames",
        anon_mb as f64 / n as f64
    );

    // The ceiling is flat in `n`: only the ring stays resident, plus one detection's working set,
    // plus a fixed baseline for the process (allocator, rendered ring is already in `resident`). If
    // detection leaked a buffer per frame, peak heap would scale with `n` and overrun this. A
    // generous 2× headroom absorbs allocator fragmentation and the sampler's coarse 2 ms cadence.
    let working_set_bytes = WORKING_SET_PLANES * plane_bytes;
    let ceiling_mb = two_x_ceiling_mb(resident_bytes, working_set_bytes);

    if measured(anon_mb, "ceiling check") {
        assert!(
            anon_mb <= ceiling_mb,
            "peak heap {anon_mb} MB exceeded the {ceiling_mb} MB ceiling (ring {ring} frames \
             resident + one detection's working set) over {n} detections — the buffer pool leaked, \
             so star detection's memory scales with the frame count instead of staying flat"
        );
        println!(
            "ceiling check OK: peak heap {anon_mb} MB ≤ {ceiling_mb} MB (flat in the {n}-frame count)"
        );
    }
}
