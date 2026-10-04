//! [`IngestConfig`]: how a run reads its frames and where it parks them.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::io::image::fits::options::FitsLoadOptions;

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
    /// Keep each decoded frame in the decode cache, for a later run on the same files to reuse.
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
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            cache_dir: default_cache_root(&|name| env::var_os(name)),
            keep_cache: false,
            memory_override: None,
            fits: FitsLoadOptions::default(),
        }
    }
}

/// The user cache directory of this platform, `lumos` beneath it, from the environment `var`
/// reads: a relative `XDG_CACHE_HOME` is ignored, as the XDG specification says.
fn default_cache_root(var: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    let absolute = |name: &str| {
        var(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let root = if cfg!(windows) {
        absolute("LOCALAPPDATA").unwrap_or_else(env::temp_dir)
    } else if cfg!(target_os = "macos") {
        absolute("HOME").map_or_else(
            || PathBuf::from("/var/tmp"),
            |home| home.join("Library/Caches"),
        )
    } else {
        absolute("XDG_CACHE_HOME")
            .or_else(|| absolute("HOME").map(|home| home.join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/var/tmp"))
    };
    root.join("lumos")
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

    /// The root follows the platform's user cache directory, and falls back to a disk-backed
    /// directory where the environment names none: on Linux `XDG_CACHE_HOME` when absolute, then
    /// `~/.cache`, then `/var/tmp`.
    #[test]
    fn default_config_spills_under_the_user_cache_directory() {
        let config = IngestConfig::default();
        assert_eq!(
            config.cache_dir,
            default_cache_root(&|name| env::var_os(name))
        );
        let root = |vars: &[(&str, &str)]| {
            default_cache_root(&|name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            })
        };
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(
                root(&[("XDG_CACHE_HOME", "/data/cache"), ("HOME", "/home/u")]),
                PathBuf::from("/data/cache/lumos")
            );
            assert_eq!(
                root(&[("XDG_CACHE_HOME", "relative"), ("HOME", "/home/u")]),
                PathBuf::from("/home/u/.cache/lumos")
            );
            assert_eq!(root(&[]), PathBuf::from("/var/tmp/lumos"));
        }
        if cfg!(target_os = "macos") {
            assert_eq!(
                root(&[("HOME", "/Users/u")]),
                PathBuf::from("/Users/u/Library/Caches/lumos")
            );
        }
        // Nothing is kept unless a caller asks, in every build profile.
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
