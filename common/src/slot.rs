use arc_swap::ArcSwapOption;
use std::sync::Arc;
use tokio::sync::Notify;

/// A lockless single-value slot for cross-thread communication.
///
/// Allows one thread to send values and another to take them without locking.
/// Only the latest value is retained; sending overwrites any previous value.
///
/// `Slot` is cheaply cloneable - all clones share the same underlying storage.
#[derive(Debug)]
pub struct Slot<T> {
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
    /// Stores a value, replacing any existing value.
    pub fn send(&self, val: T) {
        self.value.store(Some(Arc::new(val)));
        self.notify.notify_waiters();
    }

    /// Takes the value if present, leaving the slot empty.
    pub fn take(&self) -> Option<T> {
        self.value.swap(None).map(|a| Arc::into_inner(a).unwrap())
    }

    /// Returns a clone of the value if present, without removing it.
    pub fn peek(&self) -> Option<Arc<T>> {
        self.value.load_full()
    }

    /// Returns a clone of the value, waiting asynchronously if none exists.
    pub async fn peek_or_wait(&self) -> Arc<T> {
        loop {
            // Register for notification BEFORE checking value to avoid race condition
            let notified = self.notify.notified();

            if let Some(val) = self.value.load_full() {
                return val;
            }

            notified.await;
        }
    }

    /// Returns true if there is a value present.
    pub fn has_value(&self) -> bool {
        self.value.load().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_and_take() {
        let slot = Slot::default();
        assert!(!slot.has_value());

        slot.send(42);
        assert!(slot.has_value());

        let val = slot.take();
        assert_eq!(val.unwrap(), 42);
        assert!(!slot.has_value());
    }

    #[test]
    fn take_empty_returns_none() {
        let slot: Slot<i32> = Slot::default();
        assert!(slot.take().is_none());
    }

    #[test]
    fn send_overwrites_previous() {
        let slot = Slot::default();
        slot.send(1);
        slot.send(2);
        slot.send(3);

        let val = slot.take();
        assert_eq!(val.unwrap(), 3);
        assert!(slot.take().is_none());
    }

    #[test]
    fn multithreaded_send_and_take() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let sender_slot = Slot::default();
        let receiver_slot = sender_slot.clone();
        let received_count = Arc::new(AtomicUsize::new(0));
        let received_count_clone = Arc::clone(&received_count);

        let sender = thread::spawn(move || {
            for i in 0..1000 {
                sender_slot.send(i);
                thread::yield_now();
            }
        });

        let receiver = thread::spawn(move || {
            loop {
                if let Some(val) = receiver_slot.take() {
                    received_count_clone.fetch_add(1, Ordering::Relaxed);
                    if val == 999 {
                        break;
                    }
                }
                thread::yield_now();
            }
        });

        sender.join().unwrap();
        receiver.join().unwrap();

        let count = received_count.load(Ordering::Relaxed);
        assert!(count >= 1, "should have received at least one value");
        assert!(count <= 1000, "should not receive more than sent");
    }

    #[test]
    fn peek_returns_value_without_removing() {
        let slot = Slot::default();
        slot.send(42);

        let peeked = slot.peek();
        assert!(peeked.is_some());
        assert_eq!(*peeked.unwrap(), 42);

        // Value should still be there
        assert!(slot.has_value());
        let peeked_again = slot.peek();
        assert_eq!(*peeked_again.unwrap(), 42);
    }

    #[test]
    fn peek_empty_returns_none() {
        let slot: Slot<i32> = Slot::default();
        assert!(slot.peek().is_none());
    }

    #[tokio::test]
    async fn peek_or_wait_returns_immediately_if_value_exists() {
        let slot = Slot::default();
        slot.send(42);

        let val = slot.peek_or_wait().await;
        assert_eq!(*val, 42);

        // Value should still be there
        assert!(slot.has_value());
    }

    #[tokio::test]
    async fn peek_or_wait_waits_for_value() {
        let slot = Slot::default();
        let slot_clone = slot.clone();

        let handle = tokio::spawn(async move {
            let val = slot_clone.peek_or_wait().await;
            *val
        });

        // Give the task a moment to start waiting
        tokio::task::yield_now().await;

        // Value shouldn't be there yet, task should be waiting
        assert!(!slot.has_value());

        // Send the value
        slot.send(123);

        // Task should complete with the value
        let result = handle.await.unwrap();
        assert_eq!(result, 123);
    }

    #[tokio::test]
    async fn peek_or_wait_multiple_waiters() {
        let slot: Slot<i32> = Slot::default();

        let handles: Vec<_> = (0..5)
            .map(|_| {
                let slot_clone = slot.clone();
                tokio::spawn(async move {
                    let val = slot_clone.peek_or_wait().await;
                    *val
                })
            })
            .collect();

        // Give tasks time to start waiting
        tokio::task::yield_now().await;

        // Send the value - all waiters should be notified
        slot.send(999);

        // All tasks should complete with the same value
        for handle in handles {
            let result = handle.await.unwrap();
            assert_eq!(result, 999);
        }
    }
}
