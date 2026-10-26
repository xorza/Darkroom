//! [`OffscreenSignals`]: what an app on the harness asks of its host.

use std::sync::atomic::{AtomicBool, Ordering};

/// The wake and the quit an app asks of a harness host, which runs its frames itself and reads
/// these between them. Shared with the worker thread, which wakes it.
#[derive(Debug, Default)]
pub(crate) struct OffscreenSignals {
    woken: AtomicBool,
    quit: AtomicBool,
}

impl OffscreenSignals {
    /// Background work posted a result for the next frame to drain.
    pub(crate) fn note_wake(&self) {
        self.woken.store(true, Ordering::Release);
    }

    /// The app asked to end the session.
    pub(crate) fn note_quit(&self) {
        self.quit.store(true, Ordering::Release);
    }

    /// Whether a wake came since the last call; the call clears it.
    pub(crate) fn take_wake(&self) -> bool {
        self.woken.swap(false, Ordering::AcqRel)
    }

    /// Whether the app asked to quit.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the harness, which lands after this seam")
    )]
    pub(crate) fn quit_asked(&self) -> bool {
        self.quit.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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
        assert!(signals.take_wake(), "the wake shows");
        assert!(!signals.take_wake(), "and clears once taken");
        host.quit();
        assert!(
            signals.quit_asked() && signals.quit_asked(),
            "the quit stays"
        );
    }
}
