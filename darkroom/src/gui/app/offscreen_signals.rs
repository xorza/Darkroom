//! [`OffscreenSignals`]: what an app on the harness asks of its host.

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// The wake and the quit an app asks of a harness host, which runs its frames itself and reads
/// these between them. Shared with the worker thread, which wakes it.
#[derive(Debug, Default)]
pub(crate) struct OffscreenSignals {
    /// Whether a wake came that no frame took yet, behind a lock so a host can sleep until one
    /// comes.
    woken: Mutex<bool>,
    wakes: Condvar,
    quit: AtomicBool,
}

impl OffscreenSignals {
    /// Background work posted a result for the next frame to drain.
    pub(crate) fn note_wake(&self) {
        *self.woken() = true;
        self.wakes.notify_all();
    }

    /// Sleep until a wake is pending or `deadline` passes, and say whether one is.
    pub(crate) fn wait_wake(&self, deadline: Instant) -> bool {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let (woken, _) = self
            .wakes
            .wait_timeout_while(self.woken(), timeout, |woken| !*woken)
            .unwrap_or_else(PoisonError::into_inner);
        *woken
    }

    /// The app asked to end the session.
    pub(crate) fn note_quit(&self) {
        self.quit.store(true, Ordering::Release);
    }

    /// Whether a wake came since the last call; the call clears it.
    pub(crate) fn take_wake(&self) -> bool {
        mem::take(&mut *self.woken())
    }

    /// Whether a wake came that no [`Self::take_wake`] has taken yet; the call leaves it.
    pub(crate) fn wake_pending(&self) -> bool {
        *self.woken()
    }

    /// Whether the app asked to quit.
    pub(crate) fn quit_asked(&self) -> bool {
        self.quit.load(Ordering::Acquire)
    }

    /// The wake flag, whatever a panic left it as: a bool is never half written.
    fn woken(&self) -> MutexGuard<'_, bool> {
        self.woken.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use common::thread;

    use crate::gui::app::app_host::AppHost;
    use crate::gui::app::offscreen_signals::OffscreenSignals;

    /// A wake from the worker's side shows once and clears; a quit stays asked.
    #[test]
    fn an_offscreen_host_reports_wakes_and_the_quit() {
        let signals = Arc::new(OffscreenSignals::default());
        let host = AppHost::Offscreen(Arc::clone(&signals));
        let wake = host.wake();
        assert!(!signals.take_wake() && !signals.quit_asked());
        wake();
        assert!(
            signals.wake_pending() && signals.wake_pending(),
            "a look leaves the wake"
        );
        assert!(signals.take_wake(), "the wake shows");
        assert!(
            !signals.take_wake() && !signals.wake_pending(),
            "and clears once taken"
        );
        host.quit();
        assert!(
            signals.quit_asked() && signals.quit_asked(),
            "the quit stays"
        );
    }

    /// A wait with no wake ends at its deadline and says so; a wake from another thread ends it
    /// at once, and one already pending returns without a sleep.
    #[test]
    fn a_wait_ends_at_a_wake_or_its_deadline() {
        let signals = Arc::new(OffscreenSignals::default());
        assert!(!signals.wait_wake(Instant::now() + Duration::from_millis(1)));
        let waker = Arc::clone(&signals);
        let wake = thread::spawn("wake", move || waker.note_wake());
        assert!(
            signals.wait_wake(Instant::now() + Duration::from_secs(10)),
            "the wake ends the wait"
        );
        wake.join().unwrap();
        assert!(
            signals.wait_wake(Instant::now()),
            "a pending wake returns at once"
        );
        assert!(signals.take_wake());
    }
}
