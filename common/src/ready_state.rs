use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

#[derive(Debug, Clone)]
pub struct ReadyState {
    total: usize,
    count: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl ReadyState {
    pub fn new(total: usize) -> Self {
        assert!(total > 0, "ReadyState total must be positive");
        Self {
            total,
            count: Arc::new(AtomicUsize::new(0)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn signal(&self) {
        let new_count = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        if new_count == self.total {
            self.notify.notify_one();
        }
    }

    pub async fn wait(&self) {
        while self.count.load(Ordering::SeqCst) < self.total {
            self.notify.notified().await;
        }
    }
}
