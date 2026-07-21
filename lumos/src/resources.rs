const MEMORY_PERCENT: u64 = 75;

pub(crate) fn available_memory() -> u64 {
    use sysinfo::System;

    let mut system = System::new();
    system.refresh_memory();
    let available = system.available_memory();

    // macOS can report zero when compressed pages exceed free, inactive, and purgeable pages.
    if available == 0 {
        system.total_memory().saturating_sub(system.used_memory())
    } else {
        available
    }
}

pub(crate) fn memory_budget(available_memory: u64) -> u64 {
    (available_memory as u128 * MEMORY_PERCENT as u128 / 100) as u64
}

#[cfg(test)]
mod tests {
    use crate::resources::memory_budget;

    #[test]
    fn memory_budget_keeps_one_quarter_as_headroom_without_overflow() {
        assert_eq!(
            memory_budget(8 * 1024 * 1024 * 1024),
            6 * 1024 * 1024 * 1024
        );
        assert_eq!(memory_budget(u64::MAX), 13_835_058_055_282_163_711);
    }
}
