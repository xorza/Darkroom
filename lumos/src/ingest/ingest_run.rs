//! [`IngestRun`]: one run's memory reading and decode policy, taken once at its entry.

use common::CancelToken;

use crate::ingest::ingest_config::IngestConfig;
use crate::io::image::load_context::LoadContext;
use crate::memory::run_memory::RunMemory;

/// One run's memory reading and decode policy, taken once at its entry and passed to every stage,
/// so the tier decision, the chunk sizes, the decode ceiling and the FITS policy cannot disagree.
#[derive(Debug, Clone)]
pub(crate) struct IngestRun {
    pub(crate) memory: RunMemory,
    /// The decode ceiling of `memory`, the run's FITS policy, and its cancel token.
    pub(crate) context: LoadContext,
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
            },
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
        /// A run planned against `memory` rather than the system's reading, with default policy.
        pub(crate) fn planned(memory: RunMemory) -> Self {
            Self::of(memory, &IngestConfig::default(), CancelToken::never())
        }
    }
}

#[cfg(test)]
mod tests {
    use common::CancelToken;

    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::io::image::fits::options::FitsNullPolicy;
    use crate::memory::run_memory::RunMemory;

    /// The decode ceiling is 75% of the system reading whatever the override, and the FITS policy
    /// is the config's: 6000 of 8000 here, with nulls refused.
    #[test]
    fn the_run_carries_the_ceiling_and_the_policy() {
        let mut config = IngestConfig::default();
        config.fits.nulls = FitsNullPolicy::Reject;
        let run = IngestRun::of(
            RunMemory::new(8_000, Some(100)),
            &config,
            CancelToken::never(),
        );
        assert_eq!(run.memory.planning(), 100);
        assert_eq!(run.context.memory_limit_bytes, 6_000);
        assert_eq!(run.context.fits.nulls, FitsNullPolicy::Reject);
    }
}
