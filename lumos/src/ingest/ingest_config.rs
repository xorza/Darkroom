//! [`IngestConfig`]: how a run reads its frames and where it parks them.

use std::env;
use std::path::PathBuf;

use crate::io::image::fits::options::FitsLoadOptions;

/// How a run reads its frames and where it parks them: the decode policy, the memory it plans
/// against, and the frame cache every combine reads its frames through.
#[derive(Clone, Debug, PartialEq)]
pub struct IngestConfig {
    /// Root the spill files go under. A run writes only into a subdirectory it creates there, and
    /// never removes anything it did not create — see `SpillDirectory`.
    pub cache_dir: PathBuf,
    /// Keep the spill cache after stacking, for re-processing or for inspecting what spilled.
    ///
    /// Off by default: a spilled run writes every frame's planes to `cache_dir`, so leaving them
    /// behind costs the whole stack's worth of disk per run. Opt in when you mean to reuse or
    /// examine them, and delete the directory yourself.
    pub keep_cache: bool,
    /// Plan the tiers as if the machine had this many bytes available, instead of what the system
    /// reports. It never raises what one file's decode may allocate, which stays bound by the
    /// system reading.
    pub memory_override: Option<u64>,
    /// How FITS frames are read; ignored for every other format.
    pub fits: FitsLoadOptions,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            cache_dir: env::temp_dir().join("lumos_cache"),
            keep_cache: false,
            memory_override: None,
            fits: FitsLoadOptions::default(),
        }
    }
}

impl IngestConfig {
    /// Create a new cache configuration with custom cache directory.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ingest::ingest_config::*;

    #[test]
    fn default_config_spills_under_one_temp_root() {
        // Every run shares the root; each run's own subdirectory is what keeps them apart.
        let config = IngestConfig::default();
        assert_eq!(config.cache_dir, env::temp_dir().join("lumos_cache"));
        // The spill cache is cleaned up unless a caller asks otherwise, in every build profile.
        assert!(!config.keep_cache);
        assert_eq!(config.memory_override, None);
    }

    #[test]
    fn custom_cache_directory_preserves_other_defaults() {
        let directory = PathBuf::from(".tmp/custom_cache");
        let config = IngestConfig::with_cache_dir(directory.clone());

        assert_eq!(config.cache_dir, directory);
        assert!(!config.keep_cache);
        assert_eq!(config.memory_override, None);
    }
}
