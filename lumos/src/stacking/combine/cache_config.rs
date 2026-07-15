//! Cache configuration for disk-backed stacking operations.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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

    /// Get available memory, using the override when configured.
    pub fn get_available_memory(&self) -> u64 {
        self.available_memory.unwrap_or_else(get_available_memory)
    }
}

fn get_available_memory() -> u64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_memory();
    let available = sys.available_memory();

    // macOS can report zero when compressed pages exceed free, inactive, and purgeable pages.
    if available == 0 {
        sys.total_memory().saturating_sub(sys.used_memory())
    } else {
        available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_unique_process_cache_directories() {
        let first = CacheConfig::default();
        let second = CacheConfig::default();

        assert!(first.cache_dir.parent().unwrap().ends_with("lumos_cache"));
        assert_eq!(first.cache_dir.parent(), second.cache_dir.parent());
        assert_ne!(first.cache_dir, second.cache_dir);
        assert!(first.keep_cache);
        assert_eq!(first.available_memory, None);
    }

    #[test]
    fn custom_cache_directory_preserves_other_defaults() {
        let directory = PathBuf::from(".tmp/custom_cache");
        let config = CacheConfig::with_cache_dir(directory.clone());

        assert_eq!(config.cache_dir, directory);
        assert!(config.keep_cache);
        assert_eq!(config.available_memory, None);
    }

    #[test]
    fn available_memory_override_takes_precedence() {
        let config = CacheConfig {
            available_memory: Some(123_456),
            ..Default::default()
        };

        assert_eq!(config.get_available_memory(), 123_456);
    }
}
