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

    pub async fn set<T>(&self, value: T)
    where
        T: Any + Send,
    {
        let mut guard = self.inner.lock().await;
        guard.set(value);
    }
}
