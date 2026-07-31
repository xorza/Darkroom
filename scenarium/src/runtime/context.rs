use std::any::{Any, TypeId};
use std::fmt::Debug;

use ::common::CancelToken;
use hashbrown::HashMap;

use crate::execution::report::{LogEntry, LogLevel};
use crate::graph::identity::NodeId;

/// Typed handle declaring one persistent runtime context. The payload type is
/// the identity — the store is keyed by `TypeId::of::<T>()` — so a handle can
/// only yield the type it declares. Declare one handle per type; wrap two
/// contexts of the same underlying type in newtypes.
pub struct ContextType<T> {
    ctor: fn() -> T,
}

impl<T> Clone for ContextType<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ContextType<T> {}

impl<T> Debug for ContextType<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ContextType<{}>", std::any::type_name::<T>())
    }
}

impl<T: Any + Send + Sync> ContextType<T> {
    pub const fn new(ctor: fn() -> T) -> Self {
        Self { ctor }
    }
}

/// The persistent typed-resource half of the runtime context: one lazily
/// constructed payload per [`ContextType`]. Split from [`ContextManager`] so
/// resource consumers (codecs, offloaded work) receive only resources — never
/// the per-run attribution, logs, or cancellation the manager adds for lambdas.
#[derive(Debug, Default)]
pub struct ContextStore {
    store: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl ContextStore {
    /// The context of type `T`, created by the handle's constructor on first
    /// access. Infallible: every store writer keys `TypeId::of::<T>` with a
    /// `T`, so the slot cannot hold anything else.
    pub fn get<T: Any + Send + Sync>(&mut self, ctx_type: ContextType<T>) -> &mut T {
        self.store
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new((ctx_type.ctor)()))
            .downcast_mut::<T>()
            .unwrap()
    }
}

#[derive(Debug, Default)]
pub struct ContextManager {
    pub contexts: ContextStore,
    /// Node currently being invoked, set by the executor before each
    /// lambda call so `log` can attribute lines. `None` outside a run.
    pub(crate) current_node: Option<NodeId>,
    /// Log lines emitted this run, drained into `ExecutionOutcome` when the
    /// run finishes.
    pub(crate) logs: Vec<LogEntry>,
    /// The run's cooperative cancel token (the executor polls it between
    /// nodes). A lambda offloading heavy work can clone it via
    /// [`Self::cancel_flag`] and poll it inside that work to bail early.
    /// Defaults to a never-token outside a cancellable run.
    pub(crate) cancel: CancelToken,
}

impl ContextManager {
    /// Sugar for [`ContextStore::get`] on [`Self::contexts`].
    pub fn get<T: Any + Send + Sync>(&mut self, ctx_type: ContextType<T>) -> &mut T {
        self.contexts.get(ctx_type)
    }

    /// A clonable handle to the run's [`CancelToken`], for a lambda to hand to
    /// long-running work (e.g. a `spawn_blocking` lumos op) so it can poll
    /// `token.is_cancelled()` and stop early. A never-token outside a
    /// cancellable run.
    pub fn cancel_flag(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// The node this lambda is running as — the same attribution [`Self::log`]
    /// stamps its lines with. For a lambda whose effect belongs to one authored
    /// node rather than to its return value: the only thing in
    /// [`Invocation`](crate::Invocation) that says *which* node is calling.
    ///
    /// Infallible because a lambda is the only place this is reachable, and the
    /// executor stamps the node immediately before every invoke — clearing it
    /// only once the whole run loop is done. Panicking here rather than handing
    /// back an `Option` keeps that invariant stated once, instead of every
    /// lambda re-deciding what an impossible `None` should mean.
    ///
    /// [`Self::log`] reads the field directly for the opposite reason: it is
    /// *also* callable from outside a run, and a dropped log line is harmless.
    ///
    /// # Panics
    /// Outside a node invoke — on a hand-built manager, or after a run.
    pub fn current_node(&self) -> NodeId {
        self.current_node
            .expect("current_node is only readable inside a lambda invoke")
    }

    /// Emit a log line attributed to the node currently executing, and
    /// mirror it to `tracing` at the matching level so headless runs
    /// still surface output. No-op when called outside a node invoke
    /// (`current_node` unset).
    pub fn log(&mut self, level: LogLevel, msg: impl Into<String>) {
        let Some(node_id) = self.current_node else {
            return;
        };
        let message = msg.into();
        match level {
            LogLevel::Info => tracing::info!(?node_id, "{message}"),
            LogLevel::Warn => tracing::warn!(?node_id, "{message}"),
            LogLevel::Error => tracing::error!(?node_id, "{message}"),
        }
        self.logs.push(LogEntry {
            node_id,
            level,
            message,
        });
    }

    /// Sugar for [`Self::log`] at the matching level.
    pub fn info(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Info, msg);
    }
    pub fn warn(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Warn, msg);
    }
    pub fn error(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Error, msg);
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::any::{Any, TypeId};

    use crate::graph::identity::NodeId;
    use crate::runtime::context::{ContextManager, ContextStore};

    impl ContextStore {
        pub fn insert_context<T>(&mut self, value: T)
        where
            T: Any + Send + Sync,
        {
            self.store.insert(TypeId::of::<T>(), Box::new(value));
        }
    }

    impl ContextManager {
        /// Stand in for the executor's per-invoke attribution, so a lambda that
        /// reads [`ContextManager::current_node`] can be tested without a run.
        pub fn set_current_node(&mut self, node_id: NodeId) {
            self.current_node = Some(node_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::context::{ContextStore, ContextType};

    #[derive(Debug, Default)]
    struct TestCtx {
        value: i32,
    }

    #[derive(Debug, Default)]
    struct OtherCtx {
        value: i32,
    }

    const TEST_CTX: ContextType<TestCtx> = ContextType::new(TestCtx::default);
    const OTHER_CTX: ContextType<OtherCtx> = ContextType::new(OtherCtx::default);

    #[test]
    fn contexts_are_keyed_and_reused_by_payload_type() {
        let mut manager = ContextStore::default();
        let ctx = manager.get(TEST_CTX);
        assert_eq!(ctx.value, 0, "first access runs the handle's constructor");
        ctx.value = 42;
        assert_eq!(
            manager.get(TEST_CTX).value,
            42,
            "second access reuses the stored payload"
        );

        // A different payload type gets its own slot — handles cannot alias.
        let other = manager.get(OTHER_CTX);
        assert_eq!(other.value, 0);
        other.value = 7;
        assert_eq!(manager.get(TEST_CTX).value, 42);
        assert_eq!(manager.get(OTHER_CTX).value, 7);
    }
}
