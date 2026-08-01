//! [`Calls`]: how often a fixture's body ran.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{ConstValue, DynamicValue};

/// A shared count of how often the body it wraps was invoked.
///
/// "Did this node recompute, or was it served from a cache" is the question the
/// cache tests are about, and each of them grew its own counter: an
/// `Arc<AtomicUsize>` threaded into a `compute` closure, a `tokio::Mutex<i64>`,
/// a struct of three. Here it is one value, cloned into as many bodies as a
/// fixture wants and read back with [`count`](Self::count).
///
/// Cheap to clone — the count is shared, not copied — so a fixture hands the
/// same `Calls` to the graph and keeps one to assert on.
#[derive(Clone, Debug, Default)]
pub struct Calls(Arc<AtomicUsize>);

impl Calls {
    /// How many times a body of this counter has run.
    pub fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    /// Count one call by hand, for a body the combinators below have no shape
    /// for — a lambda writing a `Custom` value, which [`compute`] cannot.
    ///
    /// [`compute`]: crate::testing::graph::NodeSpec::compute
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// `body`, counted.
    pub fn counting(
        &self,
        body: impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static,
    ) -> impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static {
        let calls = self.clone();
        move |inputs| {
            calls.0.fetch_add(1, Ordering::SeqCst);
            body(inputs)
        }
    }

    /// A counted body ignoring its inputs and emitting `value` — the source
    /// every "did the upstream recompute" fixture is built on.
    ///
    /// [`NodeSpec::counted`] is this plus the declaration that carries it.
    ///
    /// [`NodeSpec::counted`]: crate::testing::graph::NodeSpec::counted
    pub fn returning(
        &self,
        value: impl Into<ConstValue>,
    ) -> impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static {
        let value = value.into();
        self.counting(move |_| value.clone())
    }

    /// A counted body emitting *its own count*, this call included — so one
    /// output says both that the node ran and how many times it has.
    pub fn tally(&self) -> impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static {
        let calls = self.clone();
        move |_| ConstValue::Int(calls.0.fetch_add(1, Ordering::SeqCst) as i64 + 1)
    }
}
