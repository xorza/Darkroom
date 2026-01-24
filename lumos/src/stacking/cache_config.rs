//! Cache configuration for memory-mapped stacking operations.
//!
//! Provides adaptive chunk sizing based on available system memory and image dimensions,
//! balancing RAM usage against disk I/O performance.

use std::path::PathBuf;

/// Minimum chunk rows to avoid excessive I/O overhead.
pub const MIN_CHUNK_ROWS: usize = 64;
/// Fraction of available memory to use (10%).
pub const MEMORY_FRACTION: u64 = 10;

/// Common configuration for cache-based stacking methods (median, sigma-clipped).
#[derive(Debug, Clone, PartialEq)]
pub struct CacheConfig {
    /// Directory for decoded image cache.
    pub cache_dir: PathBuf,
    /// Keep cache after stacking (useful for re-processing).
    pub keep_cache: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: std::env::temp_dir().join("lumos_cache"),
            keep_cache: cfg!(debug_assertions) || cfg!(test),
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
}

/// Compute optimal chunk rows based on available system memory and image dimensions.
///
/// Uses a fraction of available RAM to determine chunk size, balancing
/// memory usage against I/O performance. More available RAM = larger chunks.
///
/// # Arguments
/// * `width` - Image width in pixels
/// * `channels` - Number of color channels
/// * `frame_count` - Number of frames being stacked
///
/// # Returns
/// Chunk rows with minimum of `MIN_CHUNK_ROWS`. No upper cap - image height is the natural limit.
pub fn compute_optimal_chunk_rows(width: usize, channels: usize, frame_count: usize) -> usize {
    compute_optimal_chunk_rows_with_memory(width, channels, frame_count, get_available_memory())
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
pub fn compute_optimal_chunk_rows_with_memory(
    width: usize,
    channels: usize,
    frame_count: usize,
    available_memory: u64,
) -> usize {
    // Use ~10% of available memory for chunk processing
    let usable_memory = available_memory / MEMORY_FRACTION;

    // Bytes per row = width * channels * sizeof(f32) * frame_count
    let bytes_per_row = (width * channels * 4 * frame_count) as u64;

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

        // 10% of 8GB = 800MB
        // bytes_per_row = 6000 * 3 * 4 * 20 = 1,440,000 bytes
        // chunk_rows = 800MB / 1.44MB ≈ 596
        let bytes_per_row = 6000u64 * 3 * 4 * 20;
        let usable = available / MEMORY_FRACTION;
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
        // usable = 400MB
        // chunk_rows = 400MB / 60KB ≈ 6982 (no cap)
        let bytes_per_row = 1000u64 * 3 * 4 * 5;
        let usable = available / MEMORY_FRACTION;
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
        // usable = 1.6GB (10% of 16GB)
        // chunk_rows = 1.6GB / 9.6MB ≈ 178
        println!("Large stack (8000x3 x 100 frames, 16GB RAM): {} rows", rows);
        assert!(rows >= MIN_CHUNK_ROWS);
    }

    #[test]
    fn test_compute_optimal_rows_low_memory() {
        // Low memory: 6000x3, 20 frames, 1GB available
        let available = 1024 * 1024 * 1024u64; // 1 GB
        let rows = compute_optimal_chunk_rows_with_memory(6000, 3, 20, available);

        // usable = 100MB (10% of 1GB)
        // bytes_per_row = 1,440,000 bytes
        // chunk_rows = 100MB / 1.44MB ≈ 69
        println!("Low memory (6000x3 x 20 frames, 1GB RAM): {} rows", rows);
        assert!(rows >= MIN_CHUNK_ROWS);
    }

    #[test]
    fn test_compute_optimal_rows_very_low_memory() {
        // Very low memory: 6000x3, 20 frames, 256MB available
        let available = 256 * 1024 * 1024u64;
        let rows = compute_optimal_chunk_rows_with_memory(6000, 3, 20, available);

        // usable = 25.6MB
        // bytes_per_row = 1,440,000 bytes
        // chunk_rows = 25.6MB / 1.44MB ≈ 17, clamped to MIN
        assert_eq!(rows, MIN_CHUNK_ROWS);
        println!(
            "Very low memory (6000x3 x 20 frames, 256MB RAM): {} rows (clamped to min)",
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
        // usable = 800MB
        // chunk_rows = 800MB / 480KB ≈ 1747 (no cap)
        let bytes_per_row = 6000u64 * 4 * 20; // width * sizeof(f32) * frames (1 channel)
        let usable = available / MEMORY_FRACTION;
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
        let rows = compute_optimal_chunk_rows(width, channels, frame_count);
        let bytes_per_row = (width * channels * 4 * frame_count) as u64;
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
