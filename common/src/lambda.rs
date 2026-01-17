use std::sync::Arc;

/// A callable lambda that can be cloned and called from multiple places.
#[derive(Clone)]
pub struct Lambda(Arc<dyn Fn() + Send + Sync>);

impl Lambda {
    pub fn new<F: Fn() + Send + Sync + 'static>(f: F) -> Self {
        Self(Arc::new(f))
    }

    pub fn call(&self) {
        (self.0)()
    }
}

impl std::fmt::Debug for Lambda {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lambda").finish_non_exhaustive()
    }
}
