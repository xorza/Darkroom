use std::{future::Future, pin::Pin, sync::Arc};

use crate::common::shared_any_state::SharedAnyState;

type AsyncEventFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

pub trait AsyncEventFn: Fn(SharedAnyState) -> AsyncEventFuture + Send + Sync + 'static {}

impl<T> AsyncEventFn for T where T: Fn(SharedAnyState) -> AsyncEventFuture + Send + Sync + 'static {}

pub type AsyncEvent = dyn AsyncEventFn;

#[derive(Clone, Default)]
pub enum EventLambda {
    #[default]
    None,
    Lambda(Arc<AsyncEvent>),
}

impl EventLambda {
    pub fn new<F>(lambda: F) -> Self
    where
        F: AsyncEventFn,
    {
        Self::Lambda(Arc::new(lambda))
    }

    pub async fn invoke(&self, state: SharedAnyState) {
        match self {
            EventLambda::None => {
                panic!("Func missing lambda");
            }
            EventLambda::Lambda(inner) => (inner)(state).await,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, EventLambda::None)
    }
}

impl std::fmt::Debug for EventLambda {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventLambda::None => f.debug_struct("EventLambda::None").finish(),
            EventLambda::Lambda(_) => f.debug_struct("EventLambda::Lambda").finish(),
        }
    }
}
