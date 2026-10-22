//! [`RunStorage`]: where one run reads its frames from and writes them to.

use std::path::PathBuf;

use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::storage_tier::StorageTier;
use crate::ingest::ingest_run::IngestRun;
use crate::memory::run_plan::RunPlan;

/// Where one run reads its frames from and writes them to: the decode cache, when the run keeps
/// one and decodes files, and the scratch its tier spills to. Both live under the run's cache
/// directory, so one disk holds everything the run writes, and one check covers it.
#[derive(Debug)]
pub(crate) struct RunStorage {
    cache_dir: PathBuf,
    kept: Option<DecodeCache>,
}

/// The bytes a run writes to its disk: `kept`, the frames the decode cache does not hold yet;
/// `spilled`, every frame as its tier stores it, when the tier spills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunWrites {
    pub(crate) kept: u64,
    pub(crate) spilled: u64,
}

impl RunStorage {
    /// The storage of a run that decodes files: their decode cache opened when `run` keeps one.
    ///
    /// # Errors
    ///
    /// The decode cache cannot be opened.
    pub(crate) fn decoding(run: &IngestRun) -> Result<Self, FrameStoreError> {
        Ok(Self {
            cache_dir: run.cache_dir.clone(),
            kept: run
                .keep_cache
                .then(|| DecodeCache::open(&run.cache_dir))
                .transpose()?,
        })
    }

    /// The storage of a run whose frames the caller holds: it decodes nothing, so it keeps no
    /// decode cache.
    pub(crate) fn holding(run: &IngestRun) -> Self {
        Self {
            cache_dir: run.cache_dir.clone(),
            kept: None,
        }
    }

    /// The decode cache, when the run keeps one.
    pub(crate) const fn kept(&self) -> Option<&DecodeCache> {
        self.kept.as_ref()
    }

    /// The tier `plan` keeps the run's frames in, once the disk is known to hold what the run
    /// writes there: `writes.kept`, and `writes.spilled` when the tier spills.
    ///
    /// # Errors
    ///
    /// The scratch cannot be made, or [`FrameStoreError::InsufficientSpace`].
    pub(crate) fn tier(
        &self,
        plan: &RunPlan,
        writes: RunWrites,
    ) -> Result<StorageTier, FrameStoreError> {
        let tier = StorageTier::for_plan(plan, &self.cache_dir)?;
        match (tier.scratch(), &self.kept) {
            (Some(scratch), _) => scratch.ensure_space(writes.kept.saturating_add(writes.spilled)),
            (None, Some(kept)) => kept.ensure_space(writes.kept),
            (None, None) => {
                debug_assert_eq!(writes.kept, 0, "a run without a cache keeps nothing");
                Ok(())
            }
        }?;
        Ok(tier)
    }
}

#[cfg(test)]
mod tests {
    use common::{FileSystem, TempDir};

    use crate::frame_store::error::FrameStoreError;
    use crate::frame_store::run_storage::{RunStorage, RunWrites};
    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::memory::budget::Budget;
    use crate::memory::run_memory::RunMemory;
    use crate::memory::run_plan::RunPlan;
    use crate::memory::stage::{Stage, Units};

    /// What a run writes is checked against its disk as one: the decode cache's writes alone on a
    /// resident tier, which spills nothing, and both on a spilled one. With `free` bytes free,
    /// `share` = 0.6 · `free` kept and as much spilled each fit, and together, 1.2 · `free`, do
    /// not: the run is refused with the sum. A frame of 48 bytes fits a budget of 2³⁰ and not one
    /// of 10.
    #[test]
    fn the_disk_holds_everything_the_run_writes_or_the_run_is_refused() {
        let dir = TempDir::new("run_storage_disk");
        let ingest = IngestConfig {
            keep_cache: true,
            ..IngestConfig::with_cache_dir(dir.path().to_path_buf())
        };
        let storage =
            RunStorage::decoding(&IngestRun::planned(RunMemory::new(1 << 30, None), &ingest))
                .unwrap();
        let free = FileSystem::free_space(dir.path()).expect("the test's disk is read");
        let share = free / 10 * 6;
        let frame = [Stage {
            kept: 48,
            held: 0,
            in_flight: 0,
            units: Units::Frames,
        }];
        let resident = RunPlan::of(1, &frame, Budget::new(1 << 30, 1));
        let spilled = RunPlan::of(1, &frame, Budget::new(10, 1));
        let tier = |plan, kept, spilled| storage.tier(plan, RunWrites { kept, spilled });

        assert!(!tier(&resident, share, share).unwrap().spills());
        assert!(tier(&spilled, share, 0).unwrap().spills());
        assert!(tier(&spilled, 0, share).unwrap().spills());
        for (plan, kept, spilled) in [(&resident, 2 * share, 0), (&spilled, share, share)] {
            assert!(matches!(
                tier(plan, kept, spilled),
                Err(FrameStoreError::InsufficientSpace { needed, .. }) if needed == 2 * share
            ));
        }
    }
}
