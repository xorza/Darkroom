//! [`IngestRun`]: one run's memory reading, decode policy, storage and progress, taken once at its
//! entry.

use std::path::PathBuf;

use common::{SystemMemory, par};

use crate::ingest::ingest_config::IngestConfig;
use crate::io::load_ctx::LoadCtx;
use crate::memory::budget::Budget;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::progress_monitor::ProgressMonitor;

/// One run's memory reading, decode policy, storage and progress, taken once at its entry and
/// passed to every stage, so the tier decision, the chunk sizes, the FITS policy, the place a spill
/// goes and the place progress goes cannot disagree.
#[derive(Debug, Clone)]
pub(crate) struct IngestRun {
    pub(crate) budget: Budget,
    /// The run's FITS policy, null policy and cancel token.
    pub(crate) ctx: LoadCtx,
    /// See [`IngestConfig::cache_dir`].
    pub(crate) cache_dir: PathBuf,
    /// See [`IngestConfig::keep_cache`].
    pub(crate) keep_cache: bool,
    pub(crate) progress: ProgressCallback,
}

impl IngestRun {
    /// A run planned for the caller's override when it gave one, else the available memory the
    /// system reports now, on every thread of the pool. Several stacks run side by side each read
    /// the whole system figure; a caller that runs them so divides it through the override.
    pub(crate) fn new(config: &IngestConfig, monitor: ProgressMonitor) -> Self {
        let memory = config
            .memory_override
            .unwrap_or_else(SystemMemory::available);
        Self::of(Budget::of_memory(memory, par::threads()), config, monitor)
    }

    fn of(budget: Budget, config: &IngestConfig, monitor: ProgressMonitor) -> Self {
        let ProgressMonitor { progress, cancel } = monitor;
        Self {
            budget,
            ctx: LoadCtx {
                cancel,
                fits: config.fits.clone(),
                nulls: config.nulls,
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
        ProgressMonitor::new(self.progress.clone(), self.ctx.cancel.clone())
    }

    /// [`Self::monitor`], for the run's last stage, which takes it whole.
    pub(crate) fn into_monitor(self) -> ProgressMonitor {
        ProgressMonitor::new(self.progress, self.ctx.cancel)
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::par;

    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::memory::budget::Budget;
    use crate::progress::progress_monitor::ProgressMonitor;

    impl IngestRun {
        /// A run planned against `memory` rather than the system's reading, under `config`.
        pub(crate) fn planned(memory: u64, config: &IngestConfig) -> Self {
            Self::of(
                Budget::of_memory(memory, par::threads()),
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
    use crate::io::null_policy::NullPolicy;
    use crate::memory::budget::Budget;
    use crate::progress::progress_callback::ProgressCallback;
    use crate::progress::progress_monitor::ProgressMonitor;
    use crate::progress::stacking_progress::StackingStage;

    /// The budget is 75% of the memory the run plans for, 75 of 100 here, and a run the config
    /// gives an override of 100 plans for it in place of the system's reading. A decode takes every
    /// one of the run's three workers. The null policy and the storage are the config's: nulls
    /// refused, and the kept cache under `cache`. The monitor's halves land where the stages read
    /// them, and join again whole, borrowed or taken: its token is the decode context's, cancelled
    /// through either, and its callback hears the run's reports.
    #[test]
    fn the_run_carries_the_budget_the_policy_and_the_monitor() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let progress = ProgressCallback::new({
            let reports = Arc::clone(&reports);
            move |report| reports.lock().unwrap().push((report.cur, report.total))
        });
        let cancel = CancelToken::new();
        let mut config = IngestConfig {
            keep_cache: true,
            ..IngestConfig::with_cache_dir("cache".into())
        };
        config.nulls = NullPolicy::Reject;
        let run = IngestRun::of(
            Budget::of_memory(100, 3),
            &config,
            ProgressMonitor::new(progress, cancel.clone()),
        );
        assert_eq!(run.budget, Budget::new(75, 3));
        let overridden = IngestConfig {
            memory_override: Some(100),
            ..config
        };
        assert_eq!(
            IngestRun::new(&overridden, ProgressMonitor::unattended())
                .budget
                .usable(),
            75
        );
        assert_eq!(run.ctx.decode_threads, 3);
        assert_eq!(run.ctx.nulls, NullPolicy::Reject);
        assert_eq!(run.cache_dir, Path::new("cache"));
        assert!(run.keep_cache);

        assert!(!run.ctx.cancel.is_cancelled());
        cancel.cancel();
        assert!(run.ctx.cancel.is_cancelled());
        let monitor = run.monitor();
        assert!(monitor.cancel.is_cancelled());
        run.progress.report(1, 2, StackingStage::Loading);
        monitor.progress.report(2, 2, StackingStage::Loading);
        let whole = run.into_monitor();
        assert!(whole.cancel.is_cancelled());
        whole.progress.report(1, 1, StackingStage::Combining);
        assert_eq!(*reports.lock().unwrap(), [(1, 2), (2, 2), (1, 1)]);
    }
}
