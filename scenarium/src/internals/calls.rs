//! [`Calls`]: how often a fixture's body ran.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::data::const_value::ConstValue;
use crate::data::dynamic_value::DynamicValue;

/// A shared count of how often the body it wraps was invoked.
///
/// Answers "did this node recompute, or was it served from a cache": one value, cloned into as
/// many bodies as a fixture wants and read back with [`count`](Self::count). Cloning shares the
/// count.
#[derive(Clone, Debug, Default)]
pub struct Calls(Arc<AtomicUsize>);

impl Calls {
    /// How many times a body of this counter has run.
    pub fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }

    /// Count one call by hand, for a body the combinators below have no shape
    /// for — a lambda writing a `Custom` value, which [`compute`] cannot.
    ///
    /// [`compute`]: crate::internals::test_graph::NodeSpec::compute
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// `body`, counted.
    pub fn counting(
        &self,
        body: impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static,
    ) -> impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static {
        let calls = self.clone();
        move |inputs| {
            calls.0.fetch_add(1, Ordering::Relaxed);
            body(inputs)
        }
    }

    /// A counted body ignoring its inputs and emitting `value`.
    ///
    /// [`NodeSpec::counted`] is this plus the declaration that carries it.
    ///
    /// [`NodeSpec::counted`]: crate::internals::test_graph::NodeSpec::counted
    pub fn returning(
        &self,
        value: impl Into<ConstValue>,
    ) -> impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static {
        let value = value.into();
        self.counting(move |_| value.clone())
    }

    /// A counted body emitting *its own count*, this call included.
    pub fn tally(&self) -> impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static {
        let calls = self.clone();
        move |_| {
            ConstValue::Int(i64::try_from(calls.0.fetch_add(1, Ordering::Relaxed)).unwrap() + 1)
        }
    }
}
