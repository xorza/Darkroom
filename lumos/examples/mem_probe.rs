//! Peak-RSS probe for `calibrate_align_stack`, to validate the memory-aware (disk-tier) pipeline:
//! the disk tier should be ~flat in the frame count, the RAM tier ~linear.
//!
//! Run one config per process (peak RSS is a per-process high-water mark):
//!   LUMOS_TIER=disk LUMOS_N=8 cargo run -p lumos --release --example mem_probe
//! Uses **empty** calibration masters (identity calibration) so the high-water mark reflects the
//! align+stack work, not a masters build.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use common::{CancelToken, file_utils};
use lumos::{
    AlignStackConfig, CalibrationMasters, CalibrationSet, DEFAULT_SIGMA_THRESHOLD, RAW_EXTENSIONS,
    calibrate_align_stack,
};

/// Read a `/proc/self/status` field (e.g. `RssAnon`, `VmRSS`) in KiB.
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

fn main() {
    let n: usize = std::env::var("LUMOS_N")
        .expect("set LUMOS_N")
        .parse()
        .unwrap();
    let tier = std::env::var("LUMOS_TIER").expect("set LUMOS_TIER (ram|disk)");

    let lights_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/lumos_data/Lights");
    let lights = file_utils::files_with_extensions(&lights_dir, RAW_EXTENSIONS)
        .expect("scan RAW lights directory");
    let lights = &lights[..n.min(lights.len())];

    let empty: Vec<PathBuf> = Vec::new();
    let masters = CalibrationMasters::from_files(
        CalibrationSet {
            dark: &empty,
            flat: &empty,
            bias: &empty,
            flat_dark: &empty,
        },
        DEFAULT_SIGMA_THRESHOLD,
    )
    .expect("empty masters");

    let mut config = AlignStackConfig::default();
    config.registration.ransac.seed = Some(1);
    config.stack.cache.keep_cache = false;
    config.stack.cache.available_memory = Some(match tier.as_str() {
        "ram" => u64::MAX,
        "disk" => 1,
        other => panic!("LUMOS_TIER must be ram|disk, got {other}"),
    });

    // Sample peak anonymous (heap) RSS during the stack — the OOM-relevant metric. mmap'd file
    // pages count toward total RSS but are reclaimable; anonymous heap is not.
    let stop = Arc::new(AtomicBool::new(false));
    let peak_anon = Arc::new(AtomicU64::new(0));
    let peak_total = Arc::new(AtomicU64::new(0));
    let sampler = {
        let (stop, peak_anon, peak_total) = (stop.clone(), peak_anon.clone(), peak_total.clone());
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                peak_anon.fetch_max(status_kb("RssAnon"), Ordering::Relaxed);
                peak_total.fetch_max(status_kb("VmRSS"), Ordering::Relaxed);
                thread::sleep(Duration::from_millis(2));
            }
        })
    };

    let result =
        calibrate_align_stack(lights, &masters, &config, CancelToken::never()).expect("stack");

    stop.store(true, Ordering::Relaxed);
    sampler.join().ok();
    println!(
        "tier={tier} N={n} registered={} peak_anon_mb={} peak_total_mb={}",
        result.alignment.registered,
        peak_anon.load(Ordering::Relaxed) / 1024,
        peak_total.load(Ordering::Relaxed) / 1024,
    );
    std::hint::black_box(&result);
}
