use std::any::Any;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use common::id_type;
use hashbrown::HashMap;

type ContextCtor = dyn Fn() -> Box<dyn Any + Send> + Send + Sync;
id_type!(CtxId);

#[derive(Clone)]
pub struct ContextMeta {
    pub ctx_id: CtxId,
    pub description: String,
    ctor: Arc<ContextCtor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContextType {
    Lua,
    Custom(ContextMeta),
}

#[derive(Debug, Default)]
pub struct ContextManager {
    pub store: HashMap<ContextType, Box<dyn Any + Send>>,
}

impl Debug for ContextMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ContextMeta {{ ctx_id: {:?}, ctor: <function> }}",
            self.ctx_id
        )
    }
}
impl PartialEq for ContextMeta {
    fn eq(&self, other: &Self) -> bool {
        self.ctx_id == other.ctx_id
    }
}
impl Eq for ContextMeta {}
impl Hash for ContextMeta {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.ctx_id.hash(state);
    }
}

impl ContextMeta {
    pub fn new<T: 'static + Send + Sync, F>(ctx_id: CtxId, ctor: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        let ctor: Arc<ContextCtor> = Arc::new(move || Box::new(ctor()) as Box<dyn Any + Send>);

        ContextMeta {
            ctx_id,
            description: "".into(),
            ctor,
        }
    }

    pub fn new_default<T: 'static + Send + Sync + Default>(ctx_id: CtxId) -> Self {
        let ctor: Arc<ContextCtor> = Arc::new(|| Box::new(T::default()) as Box<dyn Any + Send>);

        ContextMeta {
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
                let value = match ctx_type {
                    ContextType::Custom(meta) => (meta.ctor)(),
                    ContextType::Lua => todo!("ContextManager missing ctor for Lua"),
                };
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
    use super::{ContextManager, ContextMeta, ContextType};

    #[derive(Debug, Default)]
    struct TestCtx {
        value: i32,
    }

    #[test]
    fn custom_default_context_is_created_and_reused() {
        let meta =
            ContextMeta::new_default::<TestCtx>("5f7dca60-37c4-4f3a-81c5-0d3d9a30c1f8".into());
        let ctx_type = ContextType::Custom(meta);

        let mut manager = ContextManager::default();
        let ctx = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx.value, 0);
        ctx.value = 42;

        let ctx_again = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx_again.value, 42);
    }

    #[test]
    fn custom_context_is_created_and_reused() {
        let meta = ContextMeta::new::<TestCtx, _>(
            "5f7dca60-37c4-4f3a-81c5-0d3d9a30c1f8".into(),
            TestCtx::default,
        );
        let ctx_type = ContextType::Custom(meta);

        let mut manager = ContextManager::default();
        let ctx = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx.value, 0);
        ctx.value = 42;

        let ctx_again = manager.get::<TestCtx>(&ctx_type);
        assert_eq!(ctx_again.value, 42);
    }
}
