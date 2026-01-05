#[derive(Debug)]
pub struct ScopeRef<F>
where
    F: FnOnce() + 'static,
{
    on_drop: Option<F>,
}
impl<F> ScopeRef<F>
where
    F: FnOnce() + 'static,
{
    pub fn new(on_drop: F) -> Self {
        Self {
            on_drop: Some(on_drop),
        }
    }
}
impl<F> Drop for ScopeRef<F>
where
    F: FnOnce() + 'static,
{
    fn drop(&mut self) {
        let on_drop = self.on_drop.take().expect("ScopeRef missing on_drop");
        on_drop();
    }
}

#[cfg(test)]
mod tests {
    use super::ScopeRef;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn drop_runs_once() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let counter = Arc::clone(&counter);
            let guard = ScopeRef::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            });
            let _ = &guard;
        }

        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "ScopeRef should invoke its closure exactly once"
        );
    }

    #[test]
    fn drop_runs_on_scope_exit() {
        let counter = Arc::new(AtomicUsize::new(0));

        let value = {
            let counter = Arc::clone(&counter);
            let guard = ScopeRef::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            });
            let _ = &guard;
            42
        };

        assert_eq!(value, 42);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "ScopeRef should invoke its closure on scope exit"
        );
    }
}
