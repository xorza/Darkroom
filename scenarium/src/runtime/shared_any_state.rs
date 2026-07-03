use std::ops::{Deref, DerefMut};

use common::Shared;
use tokio::sync::MutexGuard;

use crate::prelude::AnyState;

#[derive(Debug, Clone, Default)]
pub struct SharedAnyState {
    inner: Shared<AnyState>,
}

pub struct EventStateGuard<'a> {
    guard: MutexGuard<'a, AnyState>,
}

impl Deref for EventStateGuard<'_> {
    type Target = AnyState;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for EventStateGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl SharedAnyState {
    pub async fn lock(&self) -> EventStateGuard<'_> {
        EventStateGuard {
            guard: self.inner.lock().await,
        }
    }
}
