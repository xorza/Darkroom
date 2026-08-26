use arc_swap::ArcSwapOption;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Notify;

#[derive(Debug, Error)]
#[error("Cannot take value: other references exist")]
pub(super) struct TakeError;

/// Lockless single-value slot for cross-thread handoff.
/// Only the latest value is retained; `send` overwrites any previous value.
/// Cheap to clone — all clones share the same underlying storage.
#[derive(Debug)]
pub(super) struct Slot<T> {
    value: Arc<ArcSwapOption<T>>,
    notify: Arc<Notify>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: Arc::new(ArcSwapOption::empty()),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            notify: Arc::clone(&self.notify),
        }
    }
}

impl<T> Slot<T> {
    pub(super) fn send(&self, val: T) {
        self.value.store(Some(Arc::new(val)));
        self.notify.notify_waiters();
    }

    pub(super) async fn take_async(&self) -> Result<T, TakeError> {
        loop {
            // Register for notification BEFORE checking value to avoid lost-wakeup.
            let notified = self.notify.notified();
            if let Some(arc) = self.value.swap(None) {
                return Arc::into_inner(arc).ok_or(TakeError);
            }
            notified.await;
        }
    }

    /// Blocking counterpart of `take_async` for sync callers; parks the thread.
    pub(super) fn take_blocking(&self) -> Result<T, TakeError> {
        pollster::block_on(self.take_async())
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    impl<T> Slot<T> {
        /// Non-blocking counterpart of the waiting takes. Production only ever wants to
        /// wait, so the tests own the variant that can observe an empty slot.
        fn take(&self) -> Option<T> {
            self.value.swap(None).and_then(Arc::into_inner)
        }
    }

    #[test]
    fn send_and_take() {
        let slot = Slot::default();
        assert!(slot.take().is_none());
        slot.send(42);
        assert_eq!(slot.take().unwrap(), 42);
        assert!(slot.take().is_none());
    }

    #[test]
    fn send_overwrites_previous() {
        let slot = Slot::default();
        slot.send(1);
        slot.send(2);
        slot.send(3);
        assert_eq!(slot.take().unwrap(), 3);
        assert!(slot.take().is_none());
    }

    #[test]
    fn clones_share_storage() {
        let a = Slot::default();
        let b = a.clone();
        a.send(99);
        assert_eq!(b.take().unwrap(), 99);
        assert!(a.take().is_none());
    }

    #[test]
    fn take_blocking_waits_for_cross_thread_send() {
        let slot = Slot::default();
        let sender = thread::spawn({
            let slot = slot.clone();
            move || {
                // Give the main thread a moment to park in take_blocking.
                thread::sleep(Duration::from_millis(10));
                slot.send(5);
            }
        });
        assert_eq!(slot.take_blocking().unwrap(), 5);
        sender.join().unwrap();
    }

    #[tokio::test]
    async fn take_async_returns_immediately_if_value_exists() {
        let slot = Slot::default();
        slot.send(42);
        assert_eq!(slot.take_async().await.unwrap(), 42);
        assert!(slot.take().is_none());
    }

    #[tokio::test]
    async fn take_async_waits_for_value() {
        let slot = Slot::default();
        let clone = slot.clone();
        let handle = tokio::spawn(async move { clone.take_async().await });

        tokio::task::yield_now().await;
        slot.send(123);
        assert_eq!(handle.await.unwrap().unwrap(), 123);
    }
}
