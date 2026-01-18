use std::any::Any;

use common::Shared;

use crate::function::NodeState;

#[derive(Debug, Clone, Default)]
pub struct EventState {
    inner: Shared<NodeState>,
}

impl EventState {
    pub async fn get<T>(&self) -> Option<T>
    where
        T: Any + Send + Clone,
    {
        let guard = self.inner.lock().await;
        guard.get::<T>().cloned()
    }

    pub async fn get_or_default<T>(&self) -> T
    where
        T: Any + Send + Clone + Default,
    {
        let guard = self.inner.lock().await;
        guard.get::<T>().cloned().unwrap_or_default()
    }

    pub async fn get_or_else<T, F>(&self, f: F) -> T
    where
        T: Any + Send + Clone,
        F: FnOnce() -> T,
    {
        let guard = self.inner.lock().await;
        guard.get::<T>().cloned().unwrap_or_else(f)
    }

    pub async fn set<T>(&self, value: T)
    where
        T: Any + Send,
    {
        let mut guard = self.inner.lock().await;
        guard.set(value);
    }
}
