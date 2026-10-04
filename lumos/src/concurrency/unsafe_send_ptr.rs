//! [`UnsafeSendPtr`]: a raw pointer that may cross into Rayon closures.

/// A raw pointer that may cross into Rayon closures, for writes the caller keeps disjoint.
///
/// Only a raw pointer: wrapping any `Copy` value would make a `&Cell<_>` `Sync` from safe code.
/// `T: Send` because the writes move `T` values onto other threads.
///
/// SAFETY: Caller must ensure disjoint access from each thread.
///
/// Access the inner value via `.get()` — never `.0` — so that Edition 2024
/// closures capture `&UnsafeSendPtr` (which is Sync) rather than the inner
/// pointer field.
#[derive(Debug)]
pub(crate) struct UnsafeSendPtr<T>(*mut T);
unsafe impl<T: Send> Send for UnsafeSendPtr<T> {}
unsafe impl<T: Send> Sync for UnsafeSendPtr<T> {}

impl<T> Clone for UnsafeSendPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for UnsafeSendPtr<T> {}

impl<T> UnsafeSendPtr<T> {
    pub(crate) const fn new(ptr: *mut T) -> Self {
        Self(ptr)
    }

    pub(crate) const fn get(&self) -> *mut T {
        self.0
    }
}
