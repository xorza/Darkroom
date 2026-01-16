use std::sync::Arc;

/// A lockless single-value slot for cross-thread communication.
///
/// Allows one thread to send values and another to take them without locking.
/// Only the latest value is retained; sending overwrites any previous value.
///
/// `Slot` is cheaply cloneable - all clones share the same underlying storage.
#[derive(Debug)]
pub struct Slot<T> {
    value: Arc<Option<T>>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: Arc::new(None),
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
        self.value = Arc::new(Some(val));
    }

    /// Takes the value if present, leaving the slot empty.
    pub fn take(&mut self) -> Option<T> {
        let clone = Arc::clone(&self.value);
        self.value = Arc::new(None);

        if clone.is_some() {
            Arc::into_inner(clone).expect("Arc::into_inner failed")
        } else {
            None
        }
    }

    /// Returns true if there is a value present.
    pub fn has_value(&self) -> bool {
        self.value.is_some()
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
}
