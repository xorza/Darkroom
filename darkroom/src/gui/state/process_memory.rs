//! This process's own memory footprint, for the status bar's `MEM` clause.
//!
//! The reading is the resident set — the figure Activity Monitor and Task
//! Manager show against the process — so what the bar reports can be checked
//! against the OS. It counts everything the process holds: the graph cache the
//! bar already reports, plus decoded images, GPU driver allocations, and the
//! allocator's own retained arenas, which is why it reads well above the
//! bar's `Cache` clause.

use std::time::Duration;

use common::ResidentMemory;

/// How long a reading is reused before the next refresh. The bar renders
/// every frame; re-entering the kernel at frame rate for a number that moves
/// on a human timescale buys nothing. An idle app stops repainting entirely,
/// so the on-screen figure simply holds until something else wakes a frame.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Retained, throttled sampler for this process's resident set.
#[derive(Debug)]
pub(crate) struct ProcessMemory {
    resident: ResidentMemory,
    /// The latest reading, in bytes — `0` before the first
    /// [`Self::sample`], which is also what that call hands back inside
    /// the throttle window.
    bytes: u64,
    /// The UI time of the last reading; `None` until the first, which is what
    /// makes that first [`Self::sample`] refresh instead of leaving `bytes` at
    /// zero.
    last_sample: Option<Duration>,
}

impl ProcessMemory {
    pub(crate) fn new() -> Self {
        Self {
            resident: ResidentMemory::new(),
            bytes: 0,
            last_sample: None,
        }
    }

    /// Current resident bytes at `now`, the frame's UI time, refreshing at most
    /// once per [`SAMPLE_INTERVAL`]. A call inside that window repeats the last
    /// reading without touching the kernel. Both record passes of a frame share
    /// its `now`, so both render the same figure, and an offscreen session's
    /// stepped clock steps the readings too.
    pub(crate) fn sample(&mut self, now: Duration) -> u64 {
        if self.due(now) {
            self.bytes = self.resident.read();
        }
        self.bytes
    }

    /// Whether `now` has reached the next scheduled reading, arming the one
    /// after it. The next reading is scheduled from `now` rather than from
    /// the previous due time, so a long stall resumes at the normal cadence
    /// instead of firing a catch-up burst.
    fn due(&mut self, now: Duration) -> bool {
        let due = match self.last_sample {
            None => true,
            Some(last) => now.saturating_sub(last) >= SAMPLE_INTERVAL,
        };
        if due {
            self.last_sample = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first call always reads, and each later one only once the full
    /// interval has passed since the reading it took — not since the tick it
    /// declined.
    #[test]
    fn due_gates_readings_to_one_per_interval() {
        let mut m = ProcessMemory::new();
        let t0 = Duration::from_secs(5);
        let (interval, ms) = (SAMPLE_INTERVAL, Duration::from_millis(1));

        // Sampling at t=0: due again one interval later, then one interval
        // after that reading, and a declined tick between them must not move
        // that schedule.
        let cases = [
            (Duration::ZERO, true, "first reading"),
            (ms, false, "immediately after"),
            (
                interval
                    .checked_sub(ms)
                    .expect("the interval is longer than 1 ms"),
                false,
                "one ms short of the interval",
            ),
            (interval, true, "exactly one interval later"),
            (
                interval + interval / 2,
                false,
                "half an interval past the last reading",
            ),
            (
                interval * 2,
                true,
                "measured from the last reading, not the declined tick",
            ),
        ];
        for (offset, expected, why) in cases {
            assert_eq!(m.due(t0 + offset), expected, "t={offset:?}: {why}");
        }
    }

    /// A real reading is non-zero — this process is running — and a
    /// throttled call leaves it standing rather than zeroing it.
    #[test]
    fn sample_reports_a_live_reading_and_holds_it_between_refreshes() {
        let mut m = ProcessMemory::new();
        assert_eq!(m.bytes, 0, "no reading before the first sample");
        let t0 = Duration::from_secs(5);
        let first = m.sample(t0);
        assert!(first > 0, "this process has a resident set");
        assert_eq!(
            m.sample(t0 + Duration::from_millis(10)),
            first,
            "a throttled call repeats the last reading — what makes a \
             replayed record pass render the same figure",
        );
    }
}
