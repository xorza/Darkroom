use std::sync::Arc;

use tokio::sync::Mutex;

pub type ArcMutex<T> = Arc<Mutex<T>>;

#[derive(Clone, Debug)]
pub struct Shared<T> {
    inner: Arc<Mutex<T>>,
}

impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(value)),
        }
    }

    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, T> {
        self.inner.lock().await
    }

    pub async fn lock_owned(&self) -> tokio::sync::OwnedMutexGuard<T> {
        self.inner.clone().lock_owned().await
    }

    pub fn try_lock(&self) -> Result<tokio::sync::MutexGuard<'_, T>, tokio::sync::TryLockError> {
        self.inner.try_lock()
    }

    pub fn get_mut(&mut self) -> &mut T {
        Arc::get_mut(&mut self.inner)
            .expect("Shared::get_mut requires unique ownership of the inner Arc")
            .get_mut()
    }

    pub fn arc(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.inner)
    }
}

impl<T> std::ops::Deref for Shared<T> {
    type Target = Arc<Mutex<T>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> From<Arc<Mutex<T>>> for Shared<T> {
    fn from(inner: Arc<Mutex<T>>) -> Self {
        Self { inner }
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
