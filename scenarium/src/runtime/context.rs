use std::any::Any;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use common::CancelToken;
use common::id_type;
use hashbrown::HashMap;

use crate::execution::stats::{LogEntry, LogLevel};
use crate::graph::NodeId;

type ContextCtor = dyn Fn() -> Box<dyn Any + Send> + Send + Sync;
id_type!(CtxId);

#[derive(Clone)]
pub struct ContextType {
    ctx_id: CtxId,
    pub description: String,
    ctor: Arc<ContextCtor>,
}

#[derive(Debug, Default)]
pub struct ContextManager {
    store: HashMap<ContextType, Box<dyn Any + Send>>,
    /// Node currently being invoked, set by the executor before each
    /// lambda call so `log` can attribute lines. `None` outside a run.
    pub(crate) current_node: Option<NodeId>,
    /// Log lines emitted this run, drained into `ExecutionStats` when the
    /// run finishes.
    pub(crate) logs: Vec<LogEntry>,
    /// The run's cooperative cancel token (the executor polls it between
    /// nodes). A lambda offloading heavy work can clone it via
    /// [`Self::cancel_flag`] and poll it inside that work to bail early.
    /// Defaults to a never-token outside a cancellable run.
    pub(crate) cancel: CancelToken,
}

impl ContextManager {
    /// A clonable handle to the run's [`CancelToken`], for a lambda to hand to
    /// long-running work (e.g. a `spawn_blocking` lumos op) so it can poll
    /// `token.is_cancelled()` and stop early. A never-token outside a
    /// cancellable run.
    pub fn cancel_flag(&self) -> CancelToken {
        self.cancel.clone()
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

impl Debug for ContextType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ContextMeta {{ ctx_id: {:?}, ctor: <function> }}",
            self.ctx_id
        )
    }
}

impl PartialEq for ContextType {
    fn eq(&self, other: &Self) -> bool {
        self.ctx_id == other.ctx_id
    }
}

impl Eq for ContextType {}

impl Hash for ContextType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.ctx_id.hash(state);
    }
}

impl ContextType {
    pub fn new<T: 'static + Send + Sync, F>(ctx_id: CtxId, ctor: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        let ctor: Arc<ContextCtor> = Arc::new(move || Box::new(ctor()) as Box<dyn Any + Send>);

        ContextType {
            ctx_id,
            description: String::new(),
            ctor,
        }
    }
}

impl ContextManager {
    pub fn get<T>(&mut self, ctx_type: &ContextType) -> &mut T
    where
        T: Any + Send + Sync + 'static,
    {
        use hashbrown::hash_map::Entry;

        let boxed = match self.store.entry(ctx_type.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let value = (ctx_type.ctor)();
                entry.insert(value)
            }
        };

        boxed
            .downcast_mut::<T>()
            .expect("ContextManager has unexpected type")
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod test_support {
    use super::*;

    pub fn insert_context<T>(manager: &mut ContextManager, ctx_type: &ContextType, value: T)
    where
        T: Any + Send + Sync + 'static,
    {
        manager.store.insert(ctx_type.clone(), Box::new(value));
    }
}

#[cfg(test)]
mod tests {
    use std::{any::Any, sync::Arc};

    use crate::runtime::context::ContextCtor;

    use crate::runtime::context::{ContextManager, ContextType};

    #[derive(Debug, Default)]
    struct TestCtx {
        value: i32,
    }

    #[test]
    fn custom_default_context_is_created_and_reused() {
        let ctor: Arc<ContextCtor> =
            Arc::new(|| Box::new(TestCtx::default()) as Box<dyn Any + Send>);
        let ctx_type = ContextType {
            ctx_id: "5f7dca60-37c4-4f3a-81c5-0d3d9a30c1f8".into(),
            description: String::new(),
            ctor,
        };

        let mut manager = ContextManager::default();
        let ctx = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx.value, 0);
        ctx.value = 42;

        let ctx_again = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx_again.value, 42);
    }

    #[test]
    fn custom_context_is_created_and_reused() {
        let ctx_type = ContextType::new::<TestCtx, _>(
            "5f7dca60-37c4-4f3a-81c5-0d3d9a30c1f8".into(),
            TestCtx::default,
        );

        let mut manager = ContextManager::default();
        let ctx = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx.value, 0);
        ctx.value = 42;

        let ctx_again = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx_again.value, 42);
    }
}
