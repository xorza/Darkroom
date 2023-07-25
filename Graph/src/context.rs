use std::any::{Any, TypeId};
use std::collections::HashMap;

use common::scoped_ref::{ScopeRef, ScopeRefMut};

pub trait Context {
    fn before(&self) {}
    fn after(&self) {}

    fn before_mut(&mut self) { self.before(); }
    fn after_mut(&mut self) { self.after(); }
}

#[derive(Default)]
struct ContextManager {
    contexts: HashMap<TypeId, Box<dyn Any>>,
}


impl ContextManager {
    pub fn get<T: Context + 'static>(&self) -> Option<ScopeRef<T>> {
        self.contexts
            .get(&TypeId::of::<T>())
            .and_then(
                |any| any.downcast_ref::<T>()
            )
            .map(
                |ctx| {
                    ctx.before();

                    ScopeRef::new(
                        ctx,
                        |ctx| ctx.after(),
                    )
                }
            )
    }
    pub fn get_mut<T: Context + 'static>(&mut self) -> Option<ScopeRefMut<T>> {
        self.contexts
            .get_mut(&TypeId::of::<T>())
            .and_then(
                |any| any.downcast_mut::<T>()
            )
            .map(
                |ctx| {
                    ctx.before_mut();

                    ScopeRefMut::new(
                        ctx,
                        |ctx| ctx.after_mut(),
                    )
                }
            )
    }
    pub fn insert<T: Context + 'static>(&mut self, context: T) -> Option<T> {
        self.contexts
            .insert(
                TypeId::of::<T>(),
                Box::new(context),
            )
            .and_then(
                |any| any.downcast::<T>().ok()
            )
            .map(
                |any| *any
            )
    }
    pub fn remove<T: Context + 'static>(&mut self) -> Option<T> {
        self.contexts
            .remove(&TypeId::of::<T>())
            .and_then(
                |any| any.downcast::<T>().ok()
            )
            .map(
                |any| *any
            )
    }
}


#[cfg(test)]
mod tests {
    use std::ops::DerefMut;

    use super::*;

    impl Context for i32 {}
    impl Context for f32 {}
    impl Context for String {
        fn before_mut(&mut self) {
            self.push_str(" before");
        }
        fn after_mut(&mut self) {
            self.push_str(" after");
        }
    }

    #[test]
    fn test_context_manager() {
        let mut context_manager = ContextManager::default();

        context_manager.insert(1);
        context_manager.insert("test".to_string());
        let inserted1 = context_manager.insert(3.3f32);
        let inserted2 = context_manager.insert(4.3f32);

        assert_eq!(inserted1, None);
        assert_eq!(inserted2, Some(3.3f32));
        assert_eq!(*context_manager.get::<i32>().unwrap(), 1);
        assert_eq!(*context_manager.get::<f32>().unwrap(), 4.3f32);

        let mut s = "test before".to_string();
        assert_eq!(context_manager.get_mut::<String>().unwrap().deref_mut(), &mut s);
        let mut s = "test before after before".to_string();
        assert_eq!(context_manager.get_mut::<String>().unwrap().deref_mut(), &mut s);
    }
}
