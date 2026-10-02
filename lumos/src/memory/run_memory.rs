//! [`RunMemory`]: the memory figure one run sizes against, read once at its entry.

use common::CancelToken;

use crate::io::image::load_context::LoadContext;
use crate::memory;

/// The memory one run sizes against: one system reading, taken at the run's entry and passed to
/// every stage, so the tier decision, every chunk size and the decode ceiling cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunMemory {
    /// The available memory the system reported at the entry.
    system: u64,
    /// What the run's tier decisions and chunk sizes plan against: the caller's override when it
    /// gave one, else the system reading — and for one of several stacks run side by side, its
    /// share of that.
    planning: u64,
}

impl RunMemory {
    /// Read the system once; `memory_override` is
    /// [`CacheConfig::memory_override`](crate::CacheConfig::memory_override).
    pub(crate) fn read(memory_override: Option<u64>) -> Self {
        Self::new(memory::available_memory(), memory_override)
    }

    pub(crate) const fn new(system: u64, memory_override: Option<u64>) -> Self {
        Self {
            system,
            planning: match memory_override {
                Some(planning) => planning,
                None => system,
            },
        }
    }

    pub(crate) const fn planning(self) -> u64 {
        self.planning
    }

    /// What one file's decode may allocate. From the system reading alone: the override says how to
    /// tier, not how large a file may be.
    pub(crate) fn decode_ceiling(self) -> u64 {
        memory::memory_budget(self.system)
    }

    pub(crate) fn load_context(self, cancel: CancelToken) -> LoadContext {
        LoadContext::new(cancel, self.decode_ceiling())
    }

    /// The share of the planning figure for a stack of `part` of `whole` frames, when the stacks of
    /// all `whole` frames run side by side.
    ///
    /// Proportional to the frame count, floored, so the shares sum to at most the whole; and a
    /// stack fits its share exactly when the whole set fits the whole, so no stack spills while the
    /// total fits. `whole == 0` keeps the whole figure.
    pub(crate) fn share(self, part: usize, whole: usize) -> Self {
        if whole == 0 {
            return self;
        }
        let share = u128::from(self.planning) * part as u128 / whole as u128;
        Self {
            planning: u64::try_from(share).expect("a share is at most the whole"),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use common::CancelToken;

    use crate::memory::memory_budget;
    use crate::memory::run_memory::RunMemory;

    #[test]
    fn an_override_plans_and_the_system_reading_bounds_the_decode() {
        let plain = RunMemory::new(8_000, None);
        assert_eq!(plain.planning(), 8_000);
        assert_eq!(plain.decode_ceiling(), memory_budget(8_000));

        let overridden = RunMemory::new(8_000, Some(100));
        assert_eq!(overridden.planning(), 100);
        assert_eq!(
            overridden.decode_ceiling(),
            6_000,
            "75 % of the system reading, whatever the override"
        );
        assert_eq!(
            overridden
                .load_context(CancelToken::never())
                .memory_limit_bytes,
            6_000
        );
    }

    /// Shares are floored and sum to at most the whole: 1000 split 1 : 2 : 4 of 7 frames is
    /// ⌊1000/7⌋ = 142, ⌊2000/7⌋ = 285 and ⌊4000/7⌋ = 571, which sum to 998. A stack holding every
    /// frame gets all of it, and the decode ceiling never shrinks.
    #[test]
    fn shares_are_frame_weighted_and_never_exceed_the_whole() {
        let memory = RunMemory::new(4_000, Some(1_000));
        let shares = [1, 2, 4].map(|part| memory.share(part, 7));
        assert_eq!(shares.map(RunMemory::planning), [142, 285, 571]);
        assert!(shares.iter().map(|share| share.planning()).sum::<u64>() <= 1_000);
        assert!(
            shares
                .iter()
                .all(|share| share.decode_ceiling() == memory.decode_ceiling())
        );
        assert_eq!(memory.share(7, 7), memory);
        assert_eq!(memory.share(0, 7).planning(), 0);
        assert_eq!(memory.share(3, 0), memory);
        let huge = RunMemory::new(u64::MAX, None);
        assert_eq!(huge.share(3, 3).planning(), u64::MAX);
    }
}
