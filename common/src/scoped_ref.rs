use std::ops::{Deref, DerefMut};

/// Callback executed when the scoped reference is dropped
type DropFn<'a, T> = Box<dyn FnOnce(&T) + 'a>;
/// Callback executed when the mutable scoped reference is dropped
type DropFnMut<'a, T> = Box<dyn FnOnce(&mut T) + 'a>;

pub struct ScopeRef<'a, T: 'a> {
    data: &'a T,
    on_drop: Option<DropFn<'a, T>>,
}
pub struct ScopeRefMut<'a, T: 'a> {
    data: &'a mut T,
    on_drop: Option<DropFnMut<'a, T>>,
}

impl<'a, T> ScopeRef<'a, T> {
    pub fn new<F>(data: &'a T, on_drop: F) -> Self
    where
        F: FnOnce(&T) + 'static,
    {
        Self {
            data,
            on_drop: Some(Box::new(on_drop)),
        }
    }
}
impl<'a, T> Deref for ScopeRef<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.data
    }
}
impl<'a, T> Drop for ScopeRef<'a, T> {
    fn drop(&mut self) {
        let on_drop = self.on_drop.take().unwrap();
        (on_drop)(self.data);
    }
}

impl<'a, T> ScopeRefMut<'a, T> {
    pub fn new<F>(data: &'a mut T, on_drop: F) -> Self
    where
        F: FnOnce(&mut T) + 'static,
    {
        Self {
            data,
            on_drop: Some(Box::new(on_drop)),
        }
    }
}
impl<'a, T> Deref for ScopeRefMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.data
    }
}
impl<'a, T> DerefMut for ScopeRefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}
impl<'a, T> Drop for ScopeRefMut<'a, T> {
    fn drop(&mut self) {
        let on_drop = self.on_drop.take().unwrap();
        (on_drop)(self.data);
    }
}
