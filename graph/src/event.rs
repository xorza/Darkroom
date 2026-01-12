use std::{future::Future, pin::Pin, sync::Arc};

use crate::worker::EventLoopHandle;

type AsyncEventFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

pub trait AsyncEventFn: Fn(EventLoopHandle) -> AsyncEventFuture + Send + Sync + 'static {}

impl<T> AsyncEventFn for T where T: Fn(EventLoopHandle) -> AsyncEventFuture + Send + Sync + 'static {}

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

    pub async fn invoke(&self, event_loop_handle: EventLoopHandle) {
        match self {
            EventLambda::None => {
                panic!("Func missing lambda");
            }
            EventLambda::Lambda(inner) => (inner)(event_loop_handle).await,
        }
    }
}
