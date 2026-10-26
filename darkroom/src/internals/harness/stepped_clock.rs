//! [`SteppedClock`]: UI time that moves only when the harness steps it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use palantir::Clock;

/// A frame clock the harness advances by a fixed step per frame, so animation and click timing
/// repeat from run to run; the host holds one handle and the harness the other.
#[derive(Clone, Debug, Default)]
pub(crate) struct SteppedClock {
    nanos: Arc<AtomicU64>,
}

impl SteppedClock {
    pub(crate) fn advance(&self, dt: Duration) {
        let dt = u64::try_from(dt.as_nanos()).expect("a step of under 584 years");
        self.nanos.fetch_add(dt, Ordering::Relaxed);
    }
}

impl Clock for SteppedClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Relaxed))
    }
}
