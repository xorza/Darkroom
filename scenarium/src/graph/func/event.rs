//! [`EventLambda`]: what makes an event fire, and [`EventFires`], its fires over one event loop.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::stream::{self, Stream, StreamExt as _};

use crate::runtime::shared_any_state::SharedAnyState;

/// An event's implementation. Every event has one: it is an argument of
/// [`FuncBuilder::event`](crate::FuncBuilder::event).
#[derive(Clone)]
pub struct EventLambda(Arc<dyn Fn(SharedAnyState) -> EventFires + Send + Sync>);

/// An event's fires over one event loop, without end.
///
/// A stream that `futures_util::stream::unfold` builds from the author's future, which it holds
/// inline: one allocation for each loop start, not one for each fire. Refused: a boxed future for
/// each fire, an allocation at the event's rate; an author's loop that sends to the worker,
/// which puts the worker's sender and pause gate in the graph layer. A `Stream` is the async
/// source of many values that the tokio docs and iced's `Subscription::run` take, and arti moved
/// to `unfold` to box no future for each item
/// (<https://gitlab.torproject.org/tpo/core/arti/-/issues/2070>).
pub struct EventFires(Pin<Box<dyn Stream<Item = ()> + Send>>);

impl EventLambda {
    /// The event that fires each time a future `lambda` makes of the node's state completes, and
    /// waits for the next fire through a new one.
    pub fn new<F, Fut>(lambda: F) -> Self
    where
        F: Fn(SharedAnyState) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let lambda = Arc::new(lambda);
        Self(Arc::new(move |state| {
            let lambda = Arc::clone(&lambda);
            EventFires(Box::pin(stream::unfold(state, move |state| {
                let fire = lambda(state.clone());
                async move {
                    fire.await;
                    Some(((), state))
                }
            })))
        }))
    }

    /// The event's fires on `state`, for one event loop.
    pub fn fires(&self, state: SharedAnyState) -> EventFires {
        (self.0)(state)
    }
}

impl EventFires {
    /// Wait for the next fire.
    pub async fn next(&mut self) {
        let fire = self.0.next().await;
        debug_assert!(fire.is_some(), "an event's fires do not end");
    }
}

impl fmt::Debug for EventLambda {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EventLambda")
    }
}

impl fmt::Debug for EventFires {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EventFires")
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::{Context, Waker};

    use common::internals::alloc_audit;

    use crate::graph::func::event::{EventFires, EventLambda};
    use crate::runtime::shared_any_state::SharedAnyState;

    /// A loop start makes one allocation, the box of its fires; each fire after it makes none, as
    /// the author's future lives inline in the stream. Each fire runs a new future of the lambda
    /// on the loop's state: three fires count 3 in it.
    #[test]
    fn an_event_boxes_its_fires_once_for_each_loop_start() {
        let lambda = EventLambda::new(|state: SharedAnyState| async move {
            *state.lock().get_mut::<usize>().unwrap() += 1;
        });
        let state = SharedAnyState::default();
        state.lock().set(0_usize);
        let mut fires: Option<EventFires> = None;
        let started = alloc_audit::allocations(|| fires = Some(lambda.fires(state.clone())));
        assert_eq!(started, 1);
        let mut fires = fires.unwrap();
        let mut cx = Context::from_waker(Waker::noop());
        let fired = alloc_audit::allocations(|| {
            for _ in 0..3 {
                assert!(pin!(fires.next()).poll(&mut cx).is_ready());
            }
        });
        assert_eq!(fired, 0);
        assert_eq!(state.lock().get::<usize>(), Some(&3));
    }
}
