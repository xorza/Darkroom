//! [`IngestRun`]: one run's memory reading, decode policy and storage, taken once at its entry.

use std::path::PathBuf;

use common::CancelToken;

use crate::ingest::ingest_config::IngestConfig;
use crate::io::image::load_context::LoadContext;
use crate::memory::run_memory::RunMemory;

/// One run's memory reading, decode policy and storage, taken once at its entry and passed to
/// every stage, so the tier decision, the chunk sizes, the decode ceiling, the FITS policy and the
/// place a spill goes cannot disagree.
#[derive(Debug, Clone)]
pub(crate) struct IngestRun {
    pub(crate) memory: RunMemory,
    /// The decode ceiling of `memory`, the run's FITS policy, and its cancel token.
    pub(crate) context: LoadContext,
    /// See [`IngestConfig::cache_dir`].
    pub(crate) cache_dir: PathBuf,
    /// See [`IngestConfig::keep_cache`].
    pub(crate) keep_cache: bool,
}

impl IngestRun {
    pub(crate) fn new(config: &IngestConfig, cancel: CancelToken) -> Self {
        Self::of(RunMemory::read(config.memory_override), config, cancel)
    }

    fn of(memory: RunMemory, config: &IngestConfig, cancel: CancelToken) -> Self {
        Self {
            memory,
            context: LoadContext {
                cancel,
                memory_limit_bytes: memory.decode_ceiling(),
                fits: config.fits.clone(),
                xtrans_passes: config.xtrans_passes,
            },
            cache_dir: config.cache_dir.clone(),
            keep_cache: config.keep_cache,
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::memory::run_memory::RunMemory;

    impl IngestRun {
        /// A run planned against `memory` rather than the system's reading, under `config`.
        pub(crate) fn planned(memory: RunMemory, config: &IngestConfig) -> Self {
            Self::of(memory, config, CancelToken::never())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use common::CancelToken;

    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::io::image::fits::options::FitsNullPolicy;
    use crate::memory::run_memory::RunMemory;

    /// The decode ceiling is 75% of the system reading whatever the override, and the FITS policy
    /// and the storage are the config's: 6000 of 8000 here, with nulls refused, and the kept cache
    /// under `cache`.
    #[test]
    fn the_run_carries_the_ceiling_and_the_policy() {
        let mut config = IngestConfig {
            keep_cache: true,
            ..IngestConfig::with_cache_dir("cache".into())
        };
        config.fits.nulls = FitsNullPolicy::Reject;
        let run = IngestRun::of(
            RunMemory::new(8_000, Some(100)),
            &config,
            CancelToken::never(),
        );
        assert_eq!(run.memory.planning(), 100);
        assert_eq!(run.context.memory_limit_bytes, 6_000);
        assert_eq!(run.context.fits.nulls, FitsNullPolicy::Reject);
        assert_eq!(run.cache_dir, Path::new("cache"));
        assert!(run.keep_cache);
    }
}
