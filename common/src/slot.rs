use arc_swap::ArcSwapOption;
use std::sync::Arc;

/// A lockless single-value slot for cross-thread communication.
///
/// Allows one thread to send values and another to take them without locking.
/// Only the latest value is retained; sending overwrites any previous value.
#[derive(Debug)]
pub struct Slot<T> {
    value: ArcSwapOption<T>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Slot<T> {
    pub fn new() -> Self {
        Self {
            value: ArcSwapOption::empty(),
        }
    }

    /// Stores a value, replacing any existing value.
    pub fn send(&self, val: T) {
        self.value.store(Some(Arc::new(val)));
    }

    /// Takes the value if present, leaving the slot empty.
    pub fn take(&self) -> Option<Arc<T>> {
        self.value.swap(None)
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
        let slot = Slot::new();
        assert!(!slot.has_value());

        slot.send(42);
        assert!(slot.has_value());

        let val = slot.take();
        assert_eq!(*val.unwrap(), 42);
        assert!(!slot.has_value());
    }

    #[test]
    fn take_empty_returns_none() {
        let slot: Slot<i32> = Slot::new();
        assert!(slot.take().is_none());
    }

    #[test]
    fn send_overwrites_previous() {
        let slot = Slot::new();
        slot.send(1);
        slot.send(2);
        slot.send(3);

        let val = slot.take();
        assert_eq!(*val.unwrap(), 3);
        assert!(slot.take().is_none());
    }
}
