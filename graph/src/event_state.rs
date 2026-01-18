use std::ops::{Deref, DerefMut};

use common::Shared;
use tokio::sync::MutexGuard;

use crate::node_state::NodeState;

#[derive(Debug, Clone, Default)]
pub struct EventState {
    inner: Shared<NodeState>,
}

pub struct EventStateGuard<'a> {
    guard: MutexGuard<'a, NodeState>,
}

impl Deref for EventStateGuard<'_> {
    type Target = NodeState;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for EventStateGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl EventState {
    pub async fn lock(&self) -> EventStateGuard<'_> {
        EventStateGuard {
            guard: self.inner.lock().await,
        }
    }
}
