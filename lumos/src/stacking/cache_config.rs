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
use std::sync::Arc;

/// Minimum chunk rows to avoid excessive I/O overhead.
pub const MIN_CHUNK_ROWS: usize = 64;

/// Percentage of available memory to use for image data.
///
/// Set to 75% to balance performance (larger chunks = less I/O) against system
/// stability (leave headroom for OS and other applications). See module docs
/// for detailed rationale.
pub const MEMORY_PERCENT: u64 = 75;

/// Progress information for cache operations.
#[derive(Debug, Clone)]
pub struct CacheProgress {
    /// Current step (0-based).
    pub current: usize,
    /// Total number of steps.
    pub total: usize,
    /// Description of current operation.
    pub stage: CacheStage,
}

/// Stage of cache operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStage {
    /// Loading images into memory or writing to disk cache.
    Loading,
    /// Processing chunks during stacking.
    Processing,
}

/// Callback type for progress reporting.
pub type ProgressCallback = Arc<dyn Fn(CacheProgress) + Send + Sync>;

/// Common configuration for cache-based stacking methods (median, sigma-clipped).
#[derive(Clone)]
pub struct CacheConfig {
    /// Directory for decoded image cache.
    pub cache_dir: PathBuf,
    /// Keep cache after stacking (useful for re-processing).
    pub keep_cache: bool,
    /// Available memory override in bytes. If None, queries system for available memory.
    pub available_memory: Option<u64>,
    /// Optional progress callback.
    pub progress: Option<ProgressCallback>,
}

impl std::fmt::Debug for CacheConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheConfig")
            .field("cache_dir", &self.cache_dir)
            .field("keep_cache", &self.keep_cache)
            .field("available_memory", &self.available_memory)
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

impl PartialEq for CacheConfig {
    fn eq(&self, other: &Self) -> bool {
        self.cache_dir == other.cache_dir
            && self.keep_cache == other.keep_cache
            && self.available_memory == other.available_memory
        // Intentionally ignores `progress` callback - not comparable
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: std::env::temp_dir().join("lumos_cache"),
            keep_cache: cfg!(debug_assertions) || cfg!(test),
            available_memory: None,
            progress: None,
        }
    }
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

    /// Report progress if callback is set.
    pub fn report_progress(&self, current: usize, total: usize, stage: CacheStage) {
        if let Some(ref callback) = self.progress {
            callback(CacheProgress {
                current,
                total,
                stage,
            });
        }
    }
}

/// Get available system memory in bytes.
fn get_available_memory() -> u64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_memory();
    sys.available_memory()
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
    // Use up to 75% of available memory for chunk processing
    let usable_memory = available_memory * MEMORY_PERCENT / 100;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CacheConfig::default();
        assert!(config.cache_dir.ends_with("lumos_cache"));
        // In test mode, keep_cache should be true
        assert!(config.keep_cache);
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
