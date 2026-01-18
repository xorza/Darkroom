use std::any::Any;

#[derive(Debug, Default)]
pub struct AnyState {
    boxed: Option<Box<dyn Any + Send>>,
}

impl AnyState {
    pub fn is_none(&self) -> bool {
        self.boxed.is_none()
    }

    pub fn is_some<T>(&self) -> bool
    where
        T: Any + Send,
    {
        match &self.boxed {
            None => false,
            Some(v) => v.is::<T>(),
        }
    }

    pub fn get<T>(&self) -> Option<&T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_ref()
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_mut<T>(&mut self) -> Option<&mut T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    pub fn set<T>(&mut self, value: T)
    where
        T: Any + Send,
    {
        self.boxed = Some(Box::new(value));
    }

    pub fn get_or_default<T>(&mut self) -> &mut T
    where
        T: Any + Send + Default,
    {
        if self
            .boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
            .is_none()
        {
            self.boxed = Some(Box::<T>::default());
        }

        self.boxed.as_mut().unwrap().downcast_mut::<T>().unwrap()
    }

    pub fn get_or_default_with<T, F>(&mut self, f: F) -> &mut T
    where
        T: Any + Send,
        F: FnOnce() -> T,
    {
        if self
            .boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
            .is_none()
        {
            self.boxed = Some(Box::<T>::new(f()));
        }

        self.boxed.as_mut().unwrap().downcast_mut::<T>().unwrap()
    }
}
