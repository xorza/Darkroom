use std::sync::Arc;

use tokio::sync::Mutex;

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

    pub fn arc(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.inner)
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        Arc::get_mut(&mut self.inner).map(|mutex| mutex.get_mut())
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

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
