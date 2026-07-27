//! This process's own memory footprint, for the status bar's `MEM` clause.
//!
//! The reading is the resident set — the figure Activity Monitor and Task
//! Manager show against the process — so what the bar reports can be checked
//! against the OS. It counts everything the process holds: the graph cache the
//! bar already reports, plus decoded images, GPU driver allocations, and the
//! allocator's own retained arenas, which is why it reads well above the
//! cache's own `RAM` clause.

use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// How long a reading is reused before the next refresh. The bar renders
/// every frame; re-entering the kernel at frame rate for a number that moves
/// on a human timescale buys nothing. An idle app stops repainting entirely,
/// so the on-screen figure simply holds until something else wakes a frame.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Retained sampler for this process's resident set.
///
/// `System` is kept rather than rebuilt per reading: sysinfo carries
/// per-process bookkeeping across refreshes (on Linux it holds the `stat`
/// file open), so a fresh one each time would redo that setup for a single
/// number.
#[derive(Debug)]
pub(crate) struct ProcessMemory {
    system: System,
    pid: Pid,
    /// The latest reading, in bytes — `0` before the first
    /// [`Self::sample`], which is also what that call hands back inside
    /// the throttle window.
    bytes: u64,
    /// `None` until the first reading, which is what makes that first
    /// [`Self::sample`] refresh instead of leaving `bytes` at zero.
    last_sample: Option<Instant>,
}

impl ProcessMemory {
    pub(crate) fn new() -> Self {
        // Failure here means sysinfo has no implementation for the target at
        // all — a build-configuration error, not a runtime condition.
        let pid = sysinfo::get_current_pid().expect("sysinfo cannot resolve this platform's pid");
        Self {
            system: System::new(),
            pid,
            bytes: 0,
            last_sample: None,
        }
    }

    /// Current resident bytes, refreshing at most once per
    /// [`SAMPLE_INTERVAL`]. A call inside that window repeats the last
    /// reading without touching the kernel, which is what makes this safe
    /// to call from a replayable record pass: both passes of one frame
    /// render the same figure.
    pub(crate) fn sample(&mut self, now: Instant) -> u64 {
        if self.due(now) {
            self.system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[self.pid]),
                // We are the process being refreshed — there is no corpse to reap.
                false,
                ProcessRefreshKind::nothing().with_memory(),
            );
            self.bytes = self.system.process(self.pid).map_or(0, |p| p.memory());
        }
        self.bytes
    }

    /// Whether `now` has reached the next scheduled reading, arming the one
    /// after it. The next reading is scheduled from `now` rather than from
    /// the previous due time, so a long stall resumes at the normal cadence
    /// instead of firing a catch-up burst.
    fn due(&mut self, now: Instant) -> bool {
        let due = match self.last_sample {
            None => true,
            Some(last) => now.duration_since(last) >= SAMPLE_INTERVAL,
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
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);

        // Hand-computed against SAMPLE_INTERVAL = 1000ms, sampling at t=0:
        // due again at 1000, then (measured from 1000) at 2000, and a
        // declined tick at 1500 must not move that schedule.
        let cases = [
            (0, true, "first reading"),
            (1, false, "immediately after"),
            (999, false, "one ms short of the interval"),
            (1000, true, "exactly one interval later"),
            (1500, false, "half an interval past the last reading"),
            (2000, true, "measured from 1000, not from the declined 1500"),
        ];
        for (ms, expected, why) in cases {
            assert_eq!(m.due(at(ms)), expected, "t={ms}ms: {why}");
        }
    }

    /// A real reading is non-zero — this process is running — and a
    /// throttled call leaves it standing rather than zeroing it.
    #[test]
    fn sample_reports_a_live_reading_and_holds_it_between_refreshes() {
        let mut m = ProcessMemory::new();
        assert_eq!(m.bytes, 0, "no reading before the first sample");
        let t0 = Instant::now();
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
