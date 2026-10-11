//! [`IngestConfig`]: how a run reads its frames and where it parks them.

use std::env;
use std::path::PathBuf;

use crate::frame_store::cache_key::DecodeOptions;
use crate::io::image::fits::options::FitsLoadOptions;
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::platform::UserCache;

/// How a run reads its frames and where it parks them: the decode policy, the memory it plans
/// against, and the frame cache every combine reads its frames through.
#[derive(Clone, Debug, PartialEq)]
pub struct IngestConfig {
    /// Root of what a run writes to disk: its scratch, files with no name that vanish with the
    /// run however it ends, and with `keep_cache` the decode cache in `decode-cache/` beneath it.
    /// It must be on disk — a directory on tmpfs or ramfs is refused, as a spill there spends the
    /// memory it was meant to free. The default is the platform's user cache directory:
    /// `$XDG_CACHE_HOME/lumos`, `~/.cache/lumos`, `~/Library/Caches/lumos` on macOS,
    /// `%LOCALAPPDATA%\lumos` on Windows, with `/var/tmp/lumos` or the temporary directory where
    /// there is no home to find.
    pub cache_dir: PathBuf,
    /// Keep each decoded frame in the decode cache, with its metadata, for a later run on the same
    /// files to reuse, whether the run fits in memory or spills: the frames of a
    /// [`stack`](crate::stack) and of a calibration master, the lights of a registered stack — a
    /// RAW light as its uncalibrated mosaic, so other masters reuse it too — and the frames of a
    /// drizzle.
    ///
    /// Off by default: a kept frame costs its whole size on disk until you delete the directory.
    /// A run's own scratch is never kept either way.
    pub keep_cache: bool,
    /// Plan the tiers as if the machine had this many bytes available, instead of what the system
    /// reports. It never raises what one file's decode may allocate, which stays bound by the
    /// system reading.
    pub memory_override: Option<u64>,
    /// How FITS frames are read; ignored for every other format.
    pub fits: FitsLoadOptions,
    /// How many passes an X-Trans demosaic makes; ignored for every other mosaic.
    pub xtrans_passes: MarkesteijnPasses,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            cache_dir: UserCache::root(&|name| env::var_os(name)),
            keep_cache: false,
            memory_override: None,
            fits: FitsLoadOptions::default(),
            xtrans_passes: MarkesteijnPasses::default(),
        }
    }
}

impl IngestConfig {
    /// The options this configuration decodes every frame under.
    pub(crate) const fn decode_options(&self) -> DecodeOptions<'_> {
        DecodeOptions {
            fits: &self.fits,
            xtrans_passes: self.xtrans_passes,
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

#[cfg(test)]
mod tests {
    use crate::ingest::ingest_config::*;

    /// The root is the platform's user cache directory (see `platform::UserCache` and its tests
    /// on each system), and nothing is kept unless a caller asks, in every build profile.
    #[test]
    fn default_config_spills_under_the_user_cache_directory() {
        let config = IngestConfig::default();
        assert_eq!(config.cache_dir, UserCache::root(&|name| env::var_os(name)));
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
