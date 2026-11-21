//! [`RunStorage`]: where one run reads its frames from and writes them to.

use std::path::Path;

use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::storage_tier::StorageTier;
use crate::ingest::ingest_config::IngestConfig;
use crate::memory::run_plan::RunPlan;

/// Where one run reads its frames from and writes them to: the decode cache, when the run keeps
/// one and decodes files, and the scratch its tier spills to. Both live under the run's cache
/// directory, so one disk holds everything the run writes, and one check covers it. A view: the
/// decode cache is its file set's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunStorage<'a> {
    cache_dir: &'a Path,
    kept: Option<&'a DecodeCache>,
}

/// The bytes a run writes to its disk: `kept_bytes`, the frames the decode cache does not hold
/// yet; `spilled_bytes`, every frame as its tier stores it, when the tier spills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunWrites {
    pub(crate) kept_bytes: u64,
    pub(crate) spilled_bytes: u64,
}

impl<'a> RunStorage<'a> {
    /// The storage under `cache_dir` of a run that reads its frames through `kept`, when it keeps
    /// a decode cache.
    pub(crate) const fn new(cache_dir: &'a Path, kept: Option<&'a DecodeCache>) -> Self {
        Self { cache_dir, kept }
    }

    /// The storage of a run whose frames the caller holds: it decodes nothing, so it keeps no
    /// decode cache.
    pub(crate) fn holding(config: &'a IngestConfig) -> Self {
        Self::new(&config.cache_dir, None)
    }

    /// The decode cache, when the run keeps one.
    pub(crate) const fn kept(self) -> Option<&'a DecodeCache> {
        self.kept
    }

    /// The tier `plan` keeps the run's frames in, once the disk is known to hold what the run
    /// writes there: `writes.kept`, and `writes.spilled` when the tier spills.
    ///
    /// # Errors
    ///
    /// The scratch cannot be made, or [`FrameStoreError::InsufficientSpace`].
    pub(super) fn tier(
        self,
        plan: &RunPlan,
        writes: RunWrites,
    ) -> Result<StorageTier, FrameStoreError> {
        let tier = StorageTier::for_plan(plan, self.cache_dir)?;
        match (tier.scratch(), self.kept) {
            (Some(scratch), _) => {
                scratch.ensure_space(writes.kept_bytes.saturating_add(writes.spilled_bytes))
            }
            (None, Some(kept)) => kept.ensure_space(writes.kept_bytes),
            (None, None) => {
                debug_assert_eq!(writes.kept_bytes, 0, "a run without a cache keeps nothing");
                Ok(())
            }
        }?;
        Ok(tier)
    }
}

#[cfg(test)]
mod tests {
    use common::{FileSystem, TempDir};

    use crate::frame_store::decode_cache::DecodeCache;
    use crate::frame_store::error::FrameStoreError;
    use crate::frame_store::run_storage::{RunStorage, RunWrites};
    use crate::ingest::ingest_config::IngestConfig;
    use crate::ingest::ingest_run::IngestRun;
    use crate::memory::budget::Budget;
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
        let run = IngestRun::planned(1 << 30, &ingest);
        let cache = DecodeCache::open(&run.config.cache_dir).unwrap();
        let storage = RunStorage::new(&run.config.cache_dir, Some(&cache));
        let free = FileSystem::free_space(dir.path()).expect("the test's disk is read");
        let share = free / 10 * 6;
        let frame = [Stage {
            kept_bytes: 48,
            held_bytes: 0,
            in_flight_bytes: 0,
            units: Units::Frames,
        }];
        let resident = RunPlan::of(1, &frame, Budget::new(1 << 30, 1));
        let spilled = RunPlan::of(1, &frame, Budget::new(10, 1));
        let tier = |plan, kept, spilled| {
            storage.tier(
                plan,
                RunWrites {
                    kept_bytes: kept,
                    spilled_bytes: spilled,
                },
            )
        };

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
