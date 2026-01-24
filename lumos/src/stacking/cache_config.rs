//! Cache configuration for memory-mapped stacking operations.
//!
//! Provides adaptive chunk sizing based on available system memory,
//! balancing RAM usage against disk I/O performance.

use std::path::PathBuf;

/// Minimum chunk rows to avoid excessive I/O overhead.
pub const MIN_CHUNK_ROWS: usize = 64;
/// Maximum chunk rows to avoid excessive memory usage.
pub const MAX_CHUNK_ROWS: usize = 1024;
/// Target memory usage for chunk processing (512 MB).
pub const TARGET_CHUNK_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// Fraction of available memory to use (10%).
pub const MEMORY_FRACTION: u64 = 10;
/// Estimated bytes per row for chunk sizing calculation.
/// Based on: ~6000 width × 3 channels × 4 bytes × ~20 frames ≈ 1.4 MB/row
pub const ESTIMATED_BYTES_PER_ROW: u64 = 6000 * 3 * 4 * 20;

/// Common configuration for cache-based stacking methods (median, sigma-clipped).
#[derive(Debug, Clone, PartialEq)]
pub struct CacheConfig {
    /// Number of rows to process at once (memory vs seeks tradeoff).
    /// Higher values use more RAM but reduce disk I/O.
    pub chunk_rows: usize,
    /// Directory for decoded image cache.
    pub cache_dir: PathBuf,
    /// Keep cache after stacking (useful for re-processing).
    pub keep_cache: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            chunk_rows: compute_optimal_chunk_rows(),
            cache_dir: std::env::temp_dir().join("lumos_cache"),
            keep_cache: cfg!(debug_assertions) || cfg!(test),
        }
    }
}

impl CacheConfig {
    /// Create a new cache configuration with custom chunk rows.
    pub fn with_chunk_rows(chunk_rows: usize) -> Self {
        Self {
            chunk_rows,
            ..Default::default()
        }
    }

    /// Create a new cache configuration with custom cache directory.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            ..Default::default()
        }
    }
}

/// Compute optimal chunk rows based on available system memory.
///
/// Uses a fraction of available RAM to determine chunk size, balancing
/// memory usage against I/O performance. More available RAM = larger chunks.
///
/// # Returns
/// Chunk rows clamped between `MIN_CHUNK_ROWS` and `MAX_CHUNK_ROWS`.
pub fn compute_optimal_chunk_rows() -> usize {
    compute_optimal_chunk_rows_with_available(get_available_memory())
}

/// Get available system memory in bytes.
fn get_available_memory() -> u64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_memory();
    sys.available_memory()
}

/// Compute optimal chunk rows given available memory.
///
/// This is the core computation, separated from system calls for testability.
pub fn compute_optimal_chunk_rows_with_available(available_memory: u64) -> usize {
    // Use ~10% of available memory for chunk processing, capped at target
    let usable_memory = (available_memory / MEMORY_FRACTION).min(TARGET_CHUNK_MEMORY_BYTES);

    let chunk_rows = (usable_memory / ESTIMATED_BYTES_PER_ROW) as usize;
    let chunk_rows = chunk_rows.clamp(MIN_CHUNK_ROWS, MAX_CHUNK_ROWS);

    tracing::info!(
        available_memory_mb = available_memory / (1024 * 1024),
        usable_memory_mb = usable_memory / (1024 * 1024),
        chunk_rows,
        min_rows = MIN_CHUNK_ROWS,
        max_rows = MAX_CHUNK_ROWS,
        "Adaptive chunk sizing computed"
    );

    chunk_rows
}

/// Estimate memory usage for a given chunk configuration.
///
/// # Arguments
/// * `chunk_rows` - Number of rows per chunk
/// * `width` - Image width in pixels
/// * `channels` - Number of color channels
/// * `frame_count` - Number of frames being stacked
///
/// # Returns
/// Estimated memory usage in bytes.
#[cfg(test)]
pub fn estimate_chunk_memory(
    chunk_rows: usize,
    width: usize,
    channels: usize,
    frame_count: usize,
) -> u64 {
    let bytes_per_pixel = 4u64; // f32
    (chunk_rows as u64)
        * (width as u64)
        * (channels as u64)
        * bytes_per_pixel
        * (frame_count as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CacheConfig::default();
        assert!(config.chunk_rows >= MIN_CHUNK_ROWS);
        assert!(config.chunk_rows <= MAX_CHUNK_ROWS);
        assert!(config.cache_dir.ends_with("lumos_cache"));
        // In test mode, keep_cache should be true
        assert!(config.keep_cache);
    }

    #[test]
    fn test_with_chunk_rows() {
        let config = CacheConfig::with_chunk_rows(256);
        assert_eq!(config.chunk_rows, 256);
    }

    #[test]
    fn test_with_cache_dir() {
        let dir = PathBuf::from("/tmp/custom_cache");
        let config = CacheConfig::with_cache_dir(dir.clone());
        assert_eq!(config.cache_dir, dir);
    }

    #[test]
    fn test_compute_optimal_rows_low_memory() {
        // 1 GB available → 100 MB usable → ~69 rows → clamp to 69
        let available = 1024 * 1024 * 1024; // 1 GB
        let rows = compute_optimal_chunk_rows_with_available(available);

        let expected_usable = available / MEMORY_FRACTION; // 100 MB
        let expected_rows = (expected_usable / ESTIMATED_BYTES_PER_ROW) as usize;
        let expected_clamped = expected_rows.clamp(MIN_CHUNK_ROWS, MAX_CHUNK_ROWS);

        assert_eq!(rows, expected_clamped);
        println!("Low memory (1GB): {} rows", rows);
    }

    #[test]
    fn test_compute_optimal_rows_medium_memory() {
        // 8 GB available → 800 MB usable → capped at 512 MB → ~356 rows
        let available = 8 * 1024 * 1024 * 1024; // 8 GB
        let rows = compute_optimal_chunk_rows_with_available(available);

        // 10% of 8GB = 800MB, capped at 512MB
        let usable = (available / MEMORY_FRACTION).min(TARGET_CHUNK_MEMORY_BYTES);
        assert_eq!(usable, TARGET_CHUNK_MEMORY_BYTES);

        let expected_rows = (usable / ESTIMATED_BYTES_PER_ROW) as usize;
        assert_eq!(rows, expected_rows.clamp(MIN_CHUNK_ROWS, MAX_CHUNK_ROWS));
        println!("Medium memory (8GB): {} rows", rows);
    }

    #[test]
    fn test_compute_optimal_rows_high_memory() {
        // 64 GB available → 6.4 GB usable → capped at 512 MB → ~356 rows
        let available = 64 * 1024 * 1024 * 1024u64; // 64 GB
        let rows = compute_optimal_chunk_rows_with_available(available);

        // Should be capped at TARGET_CHUNK_MEMORY_BYTES
        let usable = (available / MEMORY_FRACTION).min(TARGET_CHUNK_MEMORY_BYTES);
        assert_eq!(usable, TARGET_CHUNK_MEMORY_BYTES);

        println!("High memory (64GB): {} rows", rows);
    }

    #[test]
    fn test_compute_optimal_rows_very_low_memory() {
        // 256 MB available → 25.6 MB usable → ~17 rows → clamp to MIN
        let available = 256 * 1024 * 1024; // 256 MB
        let rows = compute_optimal_chunk_rows_with_available(available);

        assert_eq!(rows, MIN_CHUNK_ROWS);
        println!("Very low memory (256MB): {} rows (clamped to min)", rows);
    }

    #[test]
    fn test_chunk_rows_always_in_range() {
        // Test various memory sizes
        let test_cases = [
            0u64,                     // No memory
            100 * 1024 * 1024,        // 100 MB
            512 * 1024 * 1024,        // 512 MB
            1024 * 1024 * 1024,       // 1 GB
            4 * 1024 * 1024 * 1024,   // 4 GB
            16 * 1024 * 1024 * 1024,  // 16 GB
            128 * 1024 * 1024 * 1024, // 128 GB
            u64::MAX / 2,             // Extremely high
        ];

        for available in test_cases {
            let rows = compute_optimal_chunk_rows_with_available(available);
            assert!(
                (MIN_CHUNK_ROWS..=MAX_CHUNK_ROWS).contains(&rows),
                "Chunk rows {} out of range [{}, {}] for {} bytes available",
                rows,
                MIN_CHUNK_ROWS,
                MAX_CHUNK_ROWS,
                available
            );
        }
    }

    #[test]
    fn test_estimate_chunk_memory() {
        // Typical case: 6000x4000 image, 3 channels, 20 frames, 100 rows
        let mem = estimate_chunk_memory(100, 6000, 3, 20);
        // 100 * 6000 * 3 * 4 * 20 = 144,000,000 bytes = ~137 MB
        assert_eq!(mem, 144_000_000);
        println!("Estimated memory for 100 rows: {} MB", mem / (1024 * 1024));
    }

    #[test]
    fn test_estimate_chunk_memory_matches_constant() {
        // Verify ESTIMATED_BYTES_PER_ROW matches our formula for 1 row
        let estimated = estimate_chunk_memory(1, 6000, 3, 20);
        assert_eq!(estimated, ESTIMATED_BYTES_PER_ROW);
    }

    #[test]
    fn test_current_system_allocation() {
        // This test shows actual allocation on the current system
        let available = get_available_memory();
        let rows = compute_optimal_chunk_rows();
        let estimated_mem = estimate_chunk_memory(rows, 6000, 3, 20);

        println!("=== Current System RAM Allocation ===");
        println!("Available memory: {} MB", available / (1024 * 1024));
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

        // Sanity check
        assert!(rows >= MIN_CHUNK_ROWS);
        assert!(rows <= MAX_CHUNK_ROWS);
    }
}
