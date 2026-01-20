use std::any::Any;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use common::id_type;
use hashbrown::HashMap;

type ContextCtor = dyn Fn() -> Box<dyn Any + Send> + Send + Sync;
id_type!(CtxId);

#[derive(Clone)]
pub struct ContextType {
    pub ctx_id: CtxId,
    pub description: String,
    ctor: Arc<ContextCtor>,
}

#[derive(Debug, Default)]
pub struct ContextManager {
    pub store: HashMap<ContextType, Box<dyn Any + Send>>,
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
            description: "".into(),
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

#[cfg(test)]
mod tests {
    use std::{any::Any, sync::Arc};

    use crate::context::ContextCtor;

    use super::{ContextManager, ContextType};

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
            description: "".into(),
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
