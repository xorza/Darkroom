use std::any::{Any, TypeId};
use std::collections::HashMap;

use common::is_debug;
use common::scoped_ref::ScopeRefMut;
use common::toggle::Toggle;

pub trait Context {
    fn begin_frame(&mut self) {}
    fn end_frame(&mut self) {}

    fn begin_invoke(&mut self) {}
    fn end_invoke(&mut self) {}
}

struct ContextEntry {
    context: Box<dyn Any>,
    is_active_this_frame: bool,
    ending: Option<Box<dyn FnMut(&mut ContextEntry)>>,
}

#[derive(Default)]
struct ContextManager {
    contexts: HashMap<TypeId, ContextEntry>,
    frame_started: bool,
}


impl ContextManager {
    pub fn get_mut<T: Context + 'static>(&mut self) -> Option<ScopeRefMut<T>> {
        if !self.frame_started {
            panic!("Frame not started");
        }

        let type_id = TypeId::of::<T>();
        let entry = self.contexts
            .get_mut(&type_id);
        if entry.is_none() {
            return None;
        }

        let entry = entry.unwrap();
        let was_active_this_frame = entry.is_active_this_frame.on();

        let ctx: &mut T = entry.context.downcast_mut::<T>().unwrap();

        if !was_active_this_frame {
            assert!(entry.ending.is_none());

            let ending = move |entry: &mut ContextEntry| {
                let ctx: &mut T = entry.context.downcast_mut::<T>().unwrap();
                ctx.end_frame();
            };
            entry.ending = Some(Box::new(ending));

            ctx.begin_frame();
        }

        ctx.begin_invoke();

        Some(
            ScopeRefMut::new(
                ctx,
                |ctx| ctx.end_invoke(),
            )
        )
    }
    pub fn insert<T: Context + 'static>(&mut self, context: T) {
        self.contexts
            .insert(
                TypeId::of::<T>(),
                ContextEntry {
                    context: Box::new(context),
                    is_active_this_frame: false,
                    ending: None,
                },
            );
    }

    pub fn begin_frame(&mut self) {
        if is_debug() {
            assert!(
                self.contexts
                    .values()
                    .all(
                        |entry| !entry.is_active_this_frame
                    )
            )
        }
        self.frame_started = true;
    }
    pub fn end_frame(&mut self) {
        if !self.frame_started {
            panic!("Frame not started");
        }
        self.frame_started = false;

        self.contexts
            .values_mut()
            .filter(
                |entry| entry.is_active_this_frame
            )
            .for_each(
                |entry| {
                    entry.ending.take().unwrap()(entry);
                    entry.is_active_this_frame = false;
                }
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
        fn begin_frame(&mut self) {
            self.push_str(" begin_frame");
        }
        fn end_frame(&mut self) {
            self.push_str(" end_frame");
        }

        fn begin_invoke(&mut self) {
            self.push_str(" begin_invoke");
        }
        fn end_invoke(&mut self) {
            self.push_str(" end_invoke");
        }
    }

    #[test]
    fn test_context_manager() {
        let mut context_manager = ContextManager::default();

        context_manager.insert(1);
        context_manager.insert("test".to_string());
        context_manager.insert(3.3f32);
        context_manager.insert(4.3f32);

        context_manager.begin_frame();
        assert_eq!(*context_manager.get_mut::<i32>().unwrap(), 1);
        assert_eq!(*context_manager.get_mut::<f32>().unwrap(), 4.3f32);
        context_manager.end_frame();

        context_manager.begin_frame();
        let mut s = "test begin_frame begin_invoke".to_string();
        assert_eq!(context_manager.get_mut::<String>().unwrap().deref_mut(), &mut s);
        let mut s = "test begin_frame begin_invoke end_invoke begin_invoke".to_string();
        assert_eq!(context_manager.get_mut::<String>().unwrap().deref_mut(), &mut s);
        context_manager.end_frame();

        {
            let internal_string = context_manager.contexts
                .get_mut(&TypeId::of::<String>())
                .unwrap()
                .context
                .downcast_mut::<String>()
                .unwrap();
            assert_eq!(
                internal_string.deref_mut(),
                "test begin_frame begin_invoke end_invoke begin_invoke end_invoke end_frame"
            );
        }

        context_manager.begin_frame();
        {
            let internal_string = context_manager.contexts
                .get_mut(&TypeId::of::<String>())
                .unwrap()
                .context
                .downcast_mut::<String>()
                .unwrap();
            assert_eq!(
                internal_string.deref_mut(),
                "test begin_frame begin_invoke end_invoke begin_invoke end_invoke end_frame"
            );
        }
        {
            let target =
                "test begin_frame begin_invoke end_invoke begin_invoke end_invoke end_frame begin_frame begin_invoke";
            let mut requested = context_manager
                .get_mut::<String>()
                .unwrap();
            assert_eq!(
                requested.deref_mut().deref_mut(),
                target
            );
        }
        context_manager.end_frame();
    }
}
