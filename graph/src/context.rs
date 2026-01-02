use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

type ContextCtor = dyn Fn() -> Box<dyn Any> + Send + Sync;

#[derive(Clone)]
pub struct ContextMeta {
    pub type_id: TypeId,
    pub ctor: Arc<ContextCtor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContextKind {
    Lua,
    OpenCl,
    Custom(ContextMeta),
}

impl Debug for ContextMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ContextMeta {{ type_id: {:?}, ctor: <function> }}",
            self.type_id
        )
    }
}
impl PartialEq for ContextMeta {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id
    }
}
impl Eq for ContextMeta {}
impl Hash for ContextMeta {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.type_id.hash(state);
    }
}

impl ContextMeta {
    pub fn new<T: 'static + Send + Sync>(ctor: Arc<dyn Fn() -> T + Send + Sync>) -> Self {
        let ctor: Arc<ContextCtor> = Arc::new(move || Box::new(ctor()) as Box<dyn Any>);

        ContextMeta {
            type_id: TypeId::of::<T>(),
            ctor,
        }
    }
}
