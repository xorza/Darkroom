use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::{pin::Pin, sync::Arc};

use crate::runtime::shared_any_state::SharedAnyState;

type AsyncEventFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

pub trait AsyncEventFn: Fn(SharedAnyState) -> AsyncEventFuture + Send + Sync + 'static {}

impl<T> AsyncEventFn for T where T: Fn(SharedAnyState) -> AsyncEventFuture + Send + Sync + 'static {}

pub type AsyncEvent = dyn AsyncEventFn;

/// An event's implementation. Every event has one: it is an argument of
/// [`Func::event`](crate::Func::event).
#[derive(Clone)]
pub struct EventLambda(Arc<AsyncEvent>);

impl EventLambda {
    pub fn new<F>(lambda: F) -> Self
    where
        F: AsyncEventFn,
    {
        Self(Arc::new(lambda))
    }

    pub async fn invoke(&self, state: SharedAnyState) {
        (self.0)(state).await;
    }
}

impl Debug for EventLambda {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("EventLambda")
    }
}
