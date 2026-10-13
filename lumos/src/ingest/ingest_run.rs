//! [`IngestRun`]: one run's memory reading, decode policy, storage and progress, taken once at its
//! entry.

use std::path::PathBuf;

use crate::ingest::ingest_config::IngestConfig;
use crate::io::load_context::LoadContext;
use crate::memory::budget::Budget;
use crate::memory::run_memory::RunMemory;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::progress_monitor::ProgressMonitor;

/// One run's memory reading, decode policy, storage and progress, taken once at its entry and
/// passed to every stage, so the tier decision, the chunk sizes, the decode ceiling, the FITS
/// policy, the place a spill goes and the place progress goes cannot disagree.
#[derive(Debug, Clone)]
pub(crate) struct IngestRun {
    pub(crate) budget: Budget,
    /// The decode ceiling of `memory`, the run's FITS policy, and its cancel token.
    pub(crate) context: LoadContext,
    /// See [`IngestConfig::cache_dir`].
    pub(crate) cache_dir: PathBuf,
    /// See [`IngestConfig::keep_cache`].
    pub(crate) keep_cache: bool,
    pub(crate) progress: ProgressCallback,
}

impl IngestRun {
    pub(crate) fn new(config: &IngestConfig, monitor: ProgressMonitor) -> Self {
        Self::of(
            RunMemory::read(config.memory_override),
            rayon::current_num_threads(),
            config,
            monitor,
        )
    }

    fn of(
        memory: RunMemory,
        workers: usize,
        config: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Self {
        let ProgressMonitor { progress, cancel } = monitor;
        let budget = memory.budget(workers);
        Self {
            budget,
            context: LoadContext {
                cancel,
                memory_limit_bytes: memory.decode_ceiling(),
                fits: config.fits.clone(),
                xtrans_passes: config.xtrans_passes,
                decode_threads: budget.workers(),
            },
            cache_dir: config.cache_dir.clone(),
            keep_cache: config.keep_cache,
            progress,
        }
    }

    /// The progress and cancel token the run took at its entry, joined again for a stage that
    /// keeps them both.
    pub(crate) fn monitor(&self) -> ProgressMonitor {
        ProgressMonitor::new(self.progress.clone(), self.context.cancel.clone())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::memory::run_memory::RunMemory;
    use crate::progress::progress_monitor::ProgressMonitor;

    impl IngestRun {
        /// A run planned against `memory` rather than the system's reading, under `config`.
        pub(crate) fn planned(memory: RunMemory, config: &IngestConfig) -> Self {
            Self::of(
                memory,
                rayon::current_num_threads(),
                config,
                ProgressMonitor::unattended(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use common::CancelToken;

    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::io::fits::fits_load_options::FitsNullPolicy;
    use crate::memory::budget::Budget;
    use crate::memory::run_memory::RunMemory;
    use crate::progress::progress_callback::ProgressCallback;
    use crate::progress::progress_monitor::ProgressMonitor;
    use crate::progress::stacking_progress::StackingStage;

    /// The budget is 75% of the override, and the decode ceiling 75% of the system reading whatever
    /// the override: 75 of 100 and 6000 of 8000 here. A decode takes every one of the run's three
    /// workers. The FITS policy and the storage are the config's: nulls refused, and the kept cache
    /// under `cache`. The monitor's halves land where the stages read them, and join again whole:
    /// its token is the decode context's, cancelled through either, and its callback hears the
    /// run's reports.
    #[test]
    fn the_run_carries_the_ceiling_the_policy_and_the_monitor() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let progress = ProgressCallback::new({
            let reports = Arc::clone(&reports);
            move |report| reports.lock().unwrap().push((report.current, report.total))
        });
        let cancel = CancelToken::new();
        let mut config = IngestConfig {
            keep_cache: true,
            ..IngestConfig::with_cache_dir("cache".into())
        };
        config.fits.nulls = FitsNullPolicy::Reject;
        let run = IngestRun::of(
            RunMemory::new(8_000, Some(100)),
            3,
            &config,
            ProgressMonitor::new(progress, cancel.clone()),
        );
        assert_eq!(run.budget, Budget::new(75, 3));
        assert_eq!(run.context.decode_threads, 3);
        assert_eq!(run.context.memory_limit_bytes, 6_000);
        assert_eq!(run.context.fits.nulls, FitsNullPolicy::Reject);
        assert_eq!(run.cache_dir, Path::new("cache"));
        assert!(run.keep_cache);

        assert!(!run.context.cancel.is_cancelled());
        cancel.cancel();
        assert!(run.context.cancel.is_cancelled());
        let monitor = run.monitor();
        assert!(monitor.cancel.is_cancelled());
        run.progress.report(1, 2, StackingStage::Loading);
        monitor.progress.report(2, 2, StackingStage::Loading);
        assert_eq!(*reports.lock().unwrap(), [(1, 2), (2, 2)]);
    }
}
