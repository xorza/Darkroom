//! [`RunMemory`]: the memory figure one run sizes against, read once at its entry.

use crate::memory;

/// The memory one run sizes against: one system reading, taken at the run's entry and passed to
/// every stage, so the tier decision, every chunk size and the decode ceiling cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunMemory {
    /// The available memory the system reported at the entry.
    system: u64,
    /// What the run's tier decisions and chunk sizes plan against: the caller's override when it
    /// gave one, else the system reading. Several stacks run side by side each read the whole
    /// system figure; a caller that runs them so divides it through the override.
    planning: u64,
}

impl RunMemory {
    /// Read the system once; `memory_override` is
    /// [`IngestConfig::memory_override`](crate::IngestConfig::memory_override).
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
}

#[cfg(test)]
mod tests {
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
    }
}
