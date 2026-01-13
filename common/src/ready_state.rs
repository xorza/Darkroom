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

#[cfg(test)]
mod tests {
    use super::ReadyState;
    use tokio::time::{Duration, timeout};

    #[test]
    #[should_panic(expected = "ReadyState total must be positive")]
    fn new_rejects_zero_total() {
        ReadyState::new(0);
    }

    #[tokio::test]
    async fn wait_completes_after_total_signals() {
        let ready = ReadyState::new(2);
        let waiter = tokio::spawn({
            let ready = ready.clone();
            async move {
                ready.wait().await;
            }
        });

        ready.signal();
        let first_wait = timeout(Duration::from_millis(50), ready.wait()).await;
        assert!(
            first_wait.is_err(),
            "wait should block before total signals"
        );

        ready.signal();
        let second_wait = timeout(Duration::from_millis(50), waiter).await;
        assert!(
            second_wait.is_ok(),
            "wait should complete after total signals"
        );
        second_wait.unwrap().unwrap();
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_already_ready() {
        let ready = ReadyState::new(1);
        ready.signal();

        let result = timeout(Duration::from_millis(50), ready.wait()).await;
        assert!(result.is_ok(), "wait should return when already ready");
    }
}
