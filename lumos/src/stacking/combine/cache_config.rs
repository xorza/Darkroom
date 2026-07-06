//! Cache configuration for memory-mapped stacking operations.
//!
//! Provides adaptive chunk sizing based on available system memory and image dimensions,
//! balancing RAM usage against disk I/O performance.
//!
//! # Memory Management Strategy
//!
//! The 75% memory threshold (`MEMORY_PERCENT`) is chosen as a balance between:
//!
//! - **Performance**: Using more RAM means larger chunks, fewer disk I/O operations,
//!   and better cache locality. Below ~50%, chunk sizes become too small and I/O
//!   overhead dominates.
//!
//! - **System stability**: Using >80% of available RAM risks triggering OS memory
//!   pressure, swap thrashing, or OOM kills. Other applications need headroom.
//!
//! - **Measurement accuracy**: `sysinfo::available_memory()` reports instantaneous
//!   availability which can fluctuate. A 25% buffer absorbs these variations.
//!
//! The 75% value is empirically validated across Linux/macOS/Windows to provide
//! consistent performance without memory pressure issues.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Minimum chunk rows to avoid excessive I/O overhead.
pub const MIN_CHUNK_ROWS: usize = 64;

/// Percentage of available memory to use for image data.
///
/// Set to 75% to balance performance (larger chunks = less I/O) against system
/// stability (leave headroom for OS and other applications). See module docs
/// for detailed rationale.
const MEMORY_PERCENT: u64 = 75;

/// Bytes of `available_memory` the cache may use ([`MEMORY_PERCENT`] of it). The single place the
/// budget fraction is applied — every tier calculator sizes against this.
fn memory_budget(available_memory: u64) -> u64 {
    available_memory * MEMORY_PERCENT / 100
}

/// Common configuration for cache-based stacking methods (median, sigma-clipped).
#[derive(Clone, Debug, PartialEq)]
pub struct CacheConfig {
    /// Directory for decoded image cache.
    pub cache_dir: PathBuf,
    /// Keep cache after stacking (useful for re-processing).
    pub keep_cache: bool,
    /// Available memory override in bytes. If None, queries system for available memory.
    pub available_memory: Option<u64>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: unique_cache_dir(),
            keep_cache: cfg!(debug_assertions) || cfg!(test),
            available_memory: None,
        }
    }
}

/// A process-unique cache directory `{temp}/lumos_cache/{pid}-{counter}`, so concurrent stacks never
/// share a directory: each owns its files and its `remove_dir_all` cleanup can't delete another
/// stack's still-mmapped files. The `{pid}-{counter}` suffix is unique across processes and across
/// stacks within one process. (This trades the cross-run cache reuse a fixed path would give for
/// safe concurrency — each run starts with a fresh, empty directory.)
fn unique_cache_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join("lumos_cache")
        .join(format!("{}-{}", std::process::id(), id))
}

impl CacheConfig {
    /// Create a new cache configuration with custom cache directory.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            ..Default::default()
        }
    }

    /// Get available memory - uses override if set, otherwise queries system.
    pub fn get_available_memory(&self) -> u64 {
        self.available_memory.unwrap_or_else(get_available_memory)
    }
}

/// Get available system memory in bytes.
fn get_available_memory() -> u64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_memory();
    let available = sys.available_memory();

    // sysinfo on macOS computes available_memory as:
    //   (free + inactive + purgeable - compressor_pages) * page_size
    // When compressor pages exceed the sum (common under memory pressure),
    // saturating_sub clamps to 0. Fall back to total - used in that case.
    if available == 0 {
        let total = sys.total_memory();
        let used = sys.used_memory();
        total.saturating_sub(used)
    } else {
        available
    }
}

/// Compute optimal chunk rows given available memory and image parameters.
///
/// This is the core computation, separated from system calls for testability.
/// Uses checked arithmetic to handle pathologically large datasets gracefully.
pub fn compute_optimal_chunk_rows_with_memory(
    width: usize,
    channels: usize,
    frame_count: usize,
    available_memory: u64,
) -> usize {
    let usable_memory = memory_budget(available_memory);

    // Bytes per row = width * channels * sizeof(f32) * frame_count
    // Use checked arithmetic to handle overflow gracefully
    let bytes_per_row = width
        .checked_mul(channels)
        .and_then(|v| v.checked_mul(4))
        .and_then(|v| v.checked_mul(frame_count))
        .map(|v| v as u64)
        .unwrap_or(u64::MAX); // Overflow means very large, use minimum chunk

    // Avoid division by zero for degenerate cases
    if bytes_per_row == 0 {
        return MIN_CHUNK_ROWS;
    }

    let chunk_rows = (usable_memory / bytes_per_row) as usize;
    let chunk_rows = chunk_rows.max(MIN_CHUNK_ROWS);

    tracing::info!(
        available_memory_mb = available_memory / (1024 * 1024),
        usable_memory_mb = usable_memory / (1024 * 1024),
        width,
        channels,
        frame_count,
        bytes_per_row,
        chunk_rows,
        "Adaptive chunk sizing computed"
    );

    chunk_rows
}

/// Maximum frames to decode concurrently while loading a cache tier.
///
/// Decoding is CPU-bound (libraw runs one thread per frame), so concurrency should reach the worker
/// count — but each in-flight decode transiently holds `transient_bytes_per_decode`: the decoded
/// frame *plus* its decode/stats scratch, ~2× the frame (see `decode_transient_bytes` in `cache`).
/// That sits on top of the `resident_frames` that stay resident for the rest of the load at
/// `resident_bytes_per_frame` each: every frame for the in-memory tier (they all live in RAM at
/// once), or `0` for the disk tier, which streams each decoded frame to disk and drops it.
///
/// Concurrency is the memory left after the resident set divided by the *transient* — not the
/// smaller resident frame size, which is the bug this fixes: charging one resident frame per decode
/// let peak load heap overshoot the budget ~2× (measured at 6000×6000). Result stays within the
/// usable budget (`MEMORY_PERCENT`), never exceeds `max_workers`, and is always ≥ 1.
pub fn compute_load_concurrency(
    resident_bytes_per_frame: usize,
    transient_bytes_per_decode: usize,
    resident_frames: usize,
    available_memory: u64,
    max_workers: usize,
) -> usize {
    let usable = memory_budget(available_memory);
    let transient = (transient_bytes_per_decode as u64).max(1);
    let resident = (resident_bytes_per_frame as u64).saturating_mul(resident_frames as u64);
    let headroom = usable.saturating_sub(resident);
    let mem_limit = (headroom / transient).max(1) as usize;
    mem_limit.min(max_workers.max(1))
}

/// Whether `frame_count` frames of `bytes_per_image` each fit within the usable memory budget — the
/// in-memory vs disk-backed tier decision. Overflow in the size product means "far too big" → false.
pub fn fits_in_memory(bytes_per_image: usize, frame_count: usize, available_memory: u64) -> bool {
    let Some(total_bytes) = bytes_per_image.checked_mul(frame_count) else {
        return false;
    };
    total_bytes as u64 <= memory_budget(available_memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 24 MP single-channel f32 frame (a Fuji X-Trans master frame): 24M × 4 B ≈ 96 MB.
    const FRAME_96MB: usize = 6240 * 4160 * 4;
    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn load_concurrency_is_worker_bound_when_memory_is_ample() {
        // 20 resident frames ≈ 1.9 GB against 27 GB → 20.25 GB usable, ~18 GB headroom ⇒ far more
        // than 16 decodes (at the 2× transient) fit, so the worker count is the binding limit.
        let c = compute_load_concurrency(FRAME_96MB, 2 * FRAME_96MB, 20, 27 * GB, 16);
        assert_eq!(c, 16);
    }

    #[test]
    fn load_concurrency_is_memory_bound_when_tight() {
        // Big resident set that nearly fills the usable budget leaves no headroom → serialize.
        // usable = 0.75 × 25 GB = 18.75 GB; resident = 200 × 96 MB ≈ 18.75 GB ⇒ headroom ≈ 0.
        let c = compute_load_concurrency(FRAME_96MB, 2 * FRAME_96MB, 200, 25 * GB, 16);
        assert_eq!(
            c, 1,
            "a stack that nearly fills the budget must decode one at a time"
        );
    }

    #[test]
    fn load_concurrency_charges_transient_not_resident() {
        // The core fix: concurrency divides headroom by the per-decode *transient*, not the resident
        // frame size. Disk tier (0 resident), usable = 3 GB, 1 GB frames. Charging one frame per
        // decode would allow 3 in flight (3 GB); charging the true 2× transient allows only 1.
        let one_frame = compute_load_concurrency(GB as usize, GB as usize, 0, 4 * GB, 64);
        let two_x = compute_load_concurrency(GB as usize, 2 * GB as usize, 0, 4 * GB, 64);
        assert_eq!((one_frame, two_x), (3, 1));
        assert!(
            two_x < one_frame,
            "a larger per-decode transient must lower concurrency — the overshoot fix"
        );
    }

    #[test]
    fn load_budget_is_respected_across_configs() {
        // The tiered loader's core guarantee, as an invariant sweep: for any (frame size, count,
        // budget), the chosen concurrency must not let peak *load* heap exceed the usable budget.
        // Project that peak — resident set + concurrency × the real 2× per-decode transient — and
        // assert it fits, except at the irreducible floor where not even one decode fits the budget
        // (concurrency pinned to 1). This is what the `master_stack_mem` probe measured live; here
        // it's deterministic. It fails if the divisor ever reverts to the resident frame size (the
        // ~2× overshoot bug), since concurrency would then double and the projection blow the budget.
        const MIB: u64 = 1024 * 1024;
        let workers = 32;
        for &frame_mb in &[16u64, 64, 137, 512] {
            let frame = (frame_mb * MIB) as usize;
            let transient = 2 * frame; // matches DECODE_TRANSIENT_FACTOR in `cache`
            for &count in &[4usize, 12, 24, 60] {
                for &budget_gb in &[1u64, 2, 4, 8, 16] {
                    let budget = budget_gb * 1024 * MIB;
                    let usable = memory_budget(budget);
                    let resident_frames = if fits_in_memory(frame, count, budget) {
                        count
                    } else {
                        0
                    };
                    let c = compute_load_concurrency(
                        frame,
                        transient,
                        resident_frames,
                        budget,
                        workers,
                    );
                    let projected =
                        resident_frames as u64 * frame as u64 + c as u64 * transient as u64;
                    assert!(
                        projected <= usable || c == 1,
                        "frame={frame_mb}MB count={count} budget={budget_gb}GB: concurrency {c} \
                         projects {}MB peak > {}MB usable",
                        projected / MIB,
                        usable / MIB,
                    );
                }
            }
        }
    }

    #[test]
    fn load_concurrency_disk_tier_keeps_nothing_resident() {
        // resident_frames = 0 (disk tier): only the in-flight decodes count. usable = 3 GB, at the
        // 2× transient 3 GB / 192 MB = 16 in-flight fit, so the 8-worker cap binds.
        let c = compute_load_concurrency(FRAME_96MB, 2 * FRAME_96MB, 0, 4 * GB, 8);
        assert_eq!(c, 8);
    }

    #[test]
    fn load_concurrency_large_frames_bind_below_workers() {
        // 1 GB transient per decode, disk tier, 4 GB RAM → usable 3 GB → only 3 in-flight, < 16.
        let c = compute_load_concurrency(GB as usize, GB as usize, 0, 4 * GB, 16);
        assert_eq!(c, 3);
    }

    #[test]
    fn load_concurrency_more_memory_allows_more() {
        // Parameter actually drives behavior: more RAM → strictly more concurrency, up to workers.
        let lo = compute_load_concurrency(GB as usize, GB as usize, 0, 2 * GB, 16); // usable 1.5 GB → 1
        let hi = compute_load_concurrency(GB as usize, GB as usize, 0, 8 * GB, 16); // usable 6 GB → 6
        assert_eq!((lo, hi), (1, 6));
        assert!(hi > lo);
    }

    #[test]
    fn load_concurrency_floors_and_clamps() {
        // Degenerate inputs never panic and never drop below 1.
        assert_eq!(
            compute_load_concurrency(0, 0, 0, 0, 16),
            1,
            "zero frame size → at least 1"
        );
        assert_eq!(
            compute_load_concurrency(FRAME_96MB, 2 * FRAME_96MB, 5, 0, 16),
            1,
            "no memory → at least 1"
        );
        assert_eq!(
            compute_load_concurrency(FRAME_96MB, 2 * FRAME_96MB, 5, 27 * GB, 0),
            1,
            "zero workers → at least 1"
        );
    }

    #[test]
    fn test_fits_in_memory() {
        // Test basic fit: 10 images of 1000x1000x3 = 120MB, 1GB available (750MB usable)
        assert!(fits_in_memory(1000 * 1000 * 3 * 4, 10, 1024 * 1024 * 1024));

        // Test doesn't fit: 100 images of 6000x4000x3 = 28.8GB, 16GB available (12GB usable)
        assert!(!fits_in_memory(
            6000 * 4000 * 3 * 4,
            100,
            16 * 1024 * 1024 * 1024
        ));

        // Test boundary: exactly at 75% threshold
        let bytes_per_image = 1000 * 1000 * 4;
        let frame_count = 10;
        let bytes_needed = (bytes_per_image * frame_count) as u64;
        let available_at_boundary = (bytes_needed * 100).div_ceil(75);
        assert!(fits_in_memory(
            bytes_per_image,
            frame_count,
            available_at_boundary
        ));
        assert!(!fits_in_memory(
            bytes_per_image,
            frame_count,
            available_at_boundary - 2
        ));

        // Test grayscale vs RGB with same memory
        let available = 4 * 1024 * 1024 * 1024u64; // 4GB (3GB usable)
        assert!(fits_in_memory(6000 * 4000 * 4, 20, available)); // Grayscale: 1.92GB
        assert!(!fits_in_memory(6000 * 4000 * 3 * 4, 20, available)); // RGB: 5.76GB
    }

    #[test]
    fn test_default_config() {
        let config = CacheConfig::default();
        // Cache dir is a unique subdir under `lumos_cache` (`{temp}/lumos_cache/{pid}-{counter}`).
        assert!(
            config.cache_dir.parent().unwrap().ends_with("lumos_cache"),
            "cache_dir should sit under lumos_cache, got {:?}",
            config.cache_dir
        );
        // In test mode, keep_cache should be true
        assert!(config.keep_cache);
    }

    #[test]
    fn test_default_cache_dir_is_unique_per_call() {
        // Two stacks created concurrently must not share a cache dir, or the first to finish would
        // `remove_dir_all` the other's in-flight files.
        let a = CacheConfig::default();
        let b = CacheConfig::default();
        assert_ne!(a.cache_dir, b.cache_dir);
        assert_eq!(a.cache_dir.parent(), b.cache_dir.parent());
    }

    #[test]
    fn test_with_cache_dir() {
        let dir = PathBuf::from("/tmp/custom_cache");
        let config = CacheConfig::with_cache_dir(dir.clone());
        assert_eq!(config.cache_dir, dir);
    }

    #[test]
    fn test_compute_optimal_rows_typical_image() {
        // Typical case: 6000x4000 image, 3 channels, 20 frames, 8GB available
        let available = 8 * 1024 * 1024 * 1024u64; // 8 GB
        let rows = compute_optimal_chunk_rows_with_memory(6000, 3, 20, available);

        // 75% of 8GB = 6GB
        // bytes_per_row = 6000 * 3 * 4 * 20 = 1,440,000 bytes
        // chunk_rows = 6GB / 1.44MB ≈ 4473
        let bytes_per_row = 6000u64 * 3 * 4 * 20;
        let usable = available * MEMORY_PERCENT / 100;
        let expected = (usable / bytes_per_row) as usize;
        assert_eq!(rows, expected.max(MIN_CHUNK_ROWS));
        println!("Typical image (6000x3 x 20 frames, 8GB RAM): {} rows", rows);
    }

    #[test]
    fn test_compute_optimal_rows_small_image() {
        // Small image: 1000x3, 5 frames, 4GB available
        let available = 4 * 1024 * 1024 * 1024u64;
        let rows = compute_optimal_chunk_rows_with_memory(1000, 3, 5, available);

        // bytes_per_row = 1000 * 3 * 4 * 5 = 60,000 bytes
        // usable = 3GB (75% of 4GB)
        // chunk_rows = 3GB / 60KB ≈ 52428 (no cap)
        let bytes_per_row = 1000u64 * 3 * 4 * 5;
        let usable = available * MEMORY_PERCENT / 100;
        let expected = (usable / bytes_per_row) as usize;
        assert_eq!(rows, expected);
        println!("Small image (1000x3 x 5 frames, 4GB RAM): {} rows", rows);
    }

    #[test]
    fn test_compute_optimal_rows_large_stack() {
        // Large stack: 8000x3, 100 frames, 16GB available
        let available = 16 * 1024 * 1024 * 1024u64;
        let rows = compute_optimal_chunk_rows_with_memory(8000, 3, 100, available);

        // bytes_per_row = 8000 * 3 * 4 * 100 = 9,600,000 bytes ≈ 9.6MB
        // usable = 12GB (75% of 16GB)
        // chunk_rows = 12GB / 9.6MB ≈ 1342
        println!("Large stack (8000x3 x 100 frames, 16GB RAM): {} rows", rows);
        assert!(rows >= MIN_CHUNK_ROWS);
    }

    #[test]
    fn test_compute_optimal_rows_low_memory() {
        // Low memory: 6000x3, 20 frames, 1GB available
        let available = 1024 * 1024 * 1024u64; // 1 GB
        let rows = compute_optimal_chunk_rows_with_memory(6000, 3, 20, available);

        // usable = 768MB (75% of 1GB)
        // bytes_per_row = 1,440,000 bytes
        // chunk_rows = 768MB / 1.44MB ≈ 559
        println!("Low memory (6000x3 x 20 frames, 1GB RAM): {} rows", rows);
        assert!(rows >= MIN_CHUNK_ROWS);
    }

    #[test]
    fn test_compute_optimal_rows_very_low_memory() {
        // Very low memory: 6000x3, 20 frames, 256MB available
        let available = 256 * 1024 * 1024u64;
        let rows = compute_optimal_chunk_rows_with_memory(6000, 3, 20, available);

        // usable = 192MB (75% of 256MB)
        // bytes_per_row = 1,440,000 bytes
        // chunk_rows = 192MB / 1.44MB ≈ 139
        assert!(rows >= MIN_CHUNK_ROWS);
        println!(
            "Very low memory (6000x3 x 20 frames, 256MB RAM): {} rows",
            rows
        );
    }

    #[test]
    fn test_chunk_rows_respects_minimum() {
        // Test various combinations ensure minimum is respected
        let test_cases = [
            (100, 1, 2, 0u64),                        // Degenerate
            (1000, 3, 5, 256 * 1024 * 1024),          // Small
            (6000, 3, 20, 1024 * 1024 * 1024),        // Typical, low mem
            (6000, 3, 20, 8 * 1024 * 1024 * 1024),    // Typical, high mem
            (8000, 3, 100, 16 * 1024 * 1024 * 1024),  // Large stack
            (12000, 3, 200, 64 * 1024 * 1024 * 1024), // Very large
        ];

        for (width, channels, frames, available) in test_cases {
            let rows = compute_optimal_chunk_rows_with_memory(width, channels, frames, available);
            assert!(
                rows >= MIN_CHUNK_ROWS,
                "Chunk rows {} below minimum for {}x{} x {} frames, {} bytes",
                rows,
                width,
                channels,
                frames,
                available
            );
        }
    }

    #[test]
    fn test_compute_optimal_rows_grayscale() {
        // Grayscale: 6000x1, 20 frames, 8GB available
        let available = 8 * 1024 * 1024 * 1024u64;
        let rows = compute_optimal_chunk_rows_with_memory(6000, 1, 20, available);

        // bytes_per_row = 6000 * 1 * 4 * 20 = 480,000 bytes
        // usable = 6GB (75% of 8GB)
        // chunk_rows = 6GB / 480KB ≈ 13421 (no cap)
        let bytes_per_row = 6000u64 * 4 * 20; // width * sizeof(f32) * frames (1 channel)
        let usable = available * MEMORY_PERCENT / 100;
        let expected = (usable / bytes_per_row) as usize;
        assert_eq!(rows, expected);
        println!("Grayscale (6000x1 x 20 frames, 8GB RAM): {} rows", rows);
    }

    #[test]
    fn test_current_system_allocation() {
        // This test shows actual allocation on the current system
        let width = 6000;
        let channels = 3;
        let frame_count = 20;

        let available = get_available_memory();
        let rows = compute_optimal_chunk_rows_with_memory(width, channels, frame_count, available);
        let bytes_per_row = (width * channels * 4 * frame_count) as u64; // Safe: small test values
        let estimated_mem = rows as u64 * bytes_per_row;

        println!("=== Current System RAM Allocation ===");
        println!("Available memory: {} MB", available / (1024 * 1024));
        println!("Image: {}x{} x {} frames", width, channels, frame_count);
        println!("Bytes per row: {} KB", bytes_per_row / 1024);
        println!("Computed chunk rows: {}", rows);
        println!(
            "Estimated chunk memory: {} MB",
            estimated_mem / (1024 * 1024)
        );
        println!(
            "Memory fraction used: {:.1}%",
            (estimated_mem as f64 / available as f64) * 100.0
        );
        println!("=====================================");

        assert!(rows >= MIN_CHUNK_ROWS);
    }
}
