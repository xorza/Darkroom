use arc_swap::ArcSwapOption;
use std::sync::Arc;

/// A lockless single-value slot for cross-thread communication.
///
/// Allows one thread to send values and another to take them without locking.
/// Only the latest value is retained; sending overwrites any previous value.
///
/// `Slot` is cheaply cloneable - all clones share the same underlying storage.
#[derive(Debug)]
pub struct Slot<T> {
    value: Arc<ArcSwapOption<T>>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: Arc::new(ArcSwapOption::empty()),
        }
    }
}

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
        }
    }
}

impl<T> Slot<T> {
    /// Stores a value, replacing any existing value.
    pub fn send(&mut self, val: T) {
        self.value.store(Some(Arc::new(val)));
    }

    /// Takes the value if present, leaving the slot empty.
    pub fn take(&mut self) -> Option<T> {
        let a = self.value.swap(None);
        let Some(a) = a else {
            return None;
        };

        Some(Arc::into_inner(a).unwrap())
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
        let mut slot = Slot::default();
        assert!(!slot.has_value());

        slot.send(42);
        assert!(slot.has_value());

        let val = slot.take();
        assert_eq!(val.unwrap(), 42);
        assert!(!slot.has_value());
    }

    #[test]
    fn take_empty_returns_none() {
        let mut slot: Slot<i32> = Slot::default();
        assert!(slot.take().is_none());
    }

    #[test]
    fn send_overwrites_previous() {
        let mut slot = Slot::default();
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

        let mut sender_slot = Slot::default();
        let mut receiver_slot = sender_slot.clone();
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
}
