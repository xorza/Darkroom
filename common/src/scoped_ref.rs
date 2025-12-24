use std::ops::{Deref, DerefMut};

#[derive(Debug)]
pub struct ScopeRef<'a, T, F>
where
    F: FnOnce(&T) + 'a,
{
    data: &'a T,
    on_drop: Option<F>,
}
#[derive(Debug)]
pub struct ScopeRefMut<'a, T, F>
where
    F: FnOnce(&mut T) + 'a,
{
    data: &'a mut T,
    on_drop: Option<F>,
}

impl<'a, T, F> ScopeRef<'a, T, F>
where
    F: FnOnce(&T) + 'a,
{
    pub fn new(data: &'a T, on_drop: F) -> Self {
        Self {
            data,
            on_drop: Some(on_drop),
        }
    }
}
impl<'a, T, F> Deref for ScopeRef<'a, T, F>
where
    F: FnOnce(&T) + 'a,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.data
    }
}
impl<'a, T, F> Drop for ScopeRef<'a, T, F>
where
    F: FnOnce(&T) + 'a,
{
    fn drop(&mut self) {
        let on_drop = self
            .on_drop
            .take()
            .expect("ScopeRef missing on_drop callback");
        (on_drop)(self.data);
    }
}

impl<'a, T, F> ScopeRefMut<'a, T, F>
where
    F: FnOnce(&mut T) + 'a,
{
    pub fn new(data: &'a mut T, on_drop: F) -> Self {
        Self {
            data,
            on_drop: Some(on_drop),
        }
    }
}
impl<'a, T, F> Deref for ScopeRefMut<'a, T, F>
where
    F: FnOnce(&mut T) + 'a,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.data
    }
}
impl<'a, T, F> DerefMut for ScopeRefMut<'a, T, F>
where
    F: FnOnce(&mut T) + 'a,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}
impl<'a, T, F> Drop for ScopeRefMut<'a, T, F>
where
    F: FnOnce(&mut T) + 'a,
{
    fn drop(&mut self) {
        let on_drop = self
            .on_drop
            .take()
            .expect("ScopeRefMut missing on_drop callback");
        (on_drop)(self.data);
    }
}
