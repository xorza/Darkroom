use std::sync::Arc;

use tokio::sync::{Mutex, MutexGuard};

/// `Arc<Mutex<T>>` newtype: clones share one mutex; `lock()` awaits exclusive
/// access. A plain newtype (no `Deref` to the inner `Arc`/`Mutex`) so the only
/// surface is `lock`/`get_mut` — no accidental `Arc`/`Mutex` method bleed-through.
#[derive(Debug)]
pub struct Shared<T> {
    inner: Arc<Mutex<T>>,
}

impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(value)),
        }
    }

    pub async fn lock(&self) -> MutexGuard<'_, T> {
        self.inner.lock().await
    }

    /// `&mut T` without locking when this is the sole owner; `None` if other
    /// clones exist.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        Arc::get_mut(&mut self.inner).map(|mutex| mutex.get_mut())
    }
}

impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> Default for Shared<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
