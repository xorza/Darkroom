use std::sync::Arc;

/// A shared function that can be either None or an Arc-wrapped function.
/// Generic over the function signature F which must be Send + Sync + 'static.
pub enum SharedFn<F: ?Sized + Send + Sync + 'static> {
    None,
    Some(Arc<F>),
}

impl<F: ?Sized + Send + Sync + 'static> Clone for SharedFn<F> {
    fn clone(&self) -> Self {
        match self {
            SharedFn::None => SharedFn::None,
            SharedFn::Some(f) => SharedFn::Some(Arc::clone(f)),
        }
    }
}

impl<F: ?Sized + Send + Sync + 'static> Default for SharedFn<F> {
    fn default() -> Self {
        SharedFn::None
    }
}

impl<F: ?Sized + Send + Sync + 'static> SharedFn<F> {
    pub fn new(f: Arc<F>) -> Self {
        SharedFn::Some(f)
    }

    pub fn is_none(&self) -> bool {
        matches!(self, SharedFn::None)
    }

    pub fn is_some(&self) -> bool {
        matches!(self, SharedFn::Some(_))
    }

    pub fn as_ref(&self) -> Option<&Arc<F>> {
        match self {
            SharedFn::None => None,
            SharedFn::Some(f) => Some(f),
        }
    }
}

impl<F: ?Sized + Send + Sync + 'static> std::fmt::Debug for SharedFn<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedFn::None => write!(f, "SharedFn::None"),
            SharedFn::Some(_) => write!(f, "SharedFn::Some(...)"),
        }
    }
}

impl<F: ?Sized + Send + Sync + 'static> From<Arc<F>> for SharedFn<F> {
    fn from(f: Arc<F>) -> Self {
        SharedFn::Some(f)
    }
}
