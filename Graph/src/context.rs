use std::any::{Any, TypeId};
use std::collections::HashMap;

#[derive(Default, Debug)]
struct ContextManager {
    contexts: HashMap<TypeId, Box<dyn Any>>,
}


impl ContextManager {
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.contexts
            .get(&TypeId::of::<T>())
            .and_then(
                |any| any.downcast_ref::<T>()
            )
    }
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.contexts
            .get_mut(&TypeId::of::<T>())
            .and_then(
                |any| any.downcast_mut::<T>()
            )
    }
    pub fn insert<T: 'static>(&mut self, context: T) -> Option<T> {
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
    pub fn remove<T: 'static>(&mut self) -> Option<T> {
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
    use super::*;

    #[test]
    fn test_context_manager() {
        let mut context_manager = ContextManager::default();

        context_manager.insert(1);
        context_manager.insert("test".to_string());
        let inserted1 = context_manager.insert(3.3f32);
        let inserted2 = context_manager.insert(4.3f32);

        assert_eq!(inserted1, None);
        assert_eq!(inserted2, Some(3.3f32));
        assert_eq!(context_manager.get::<i32>(), Some(&1));
        assert_eq!(context_manager.get::<f32>(), Some(&4.3f32));

        let mut s = "test".to_string();
        assert_eq!(context_manager.get_mut::<String>(), Some(&mut s));
    }
}
