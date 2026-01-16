use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// A synchronization primitive that allows one side to pause waiters.
///
/// The controller can pause/resume, and waiters will block when paused.
/// Multiple waiters can proceed concurrently when not paused.
/// Closing the gate is instant and does not wait for current waiters.
#[derive(Debug)]
struct PauseGateInner {
    closed: AtomicBool,
    notify: Notify,
}

#[derive(Debug, Clone)]
pub struct PauseGate {
    inner: Arc<PauseGateInner>,
}

impl Default for PauseGate {
    fn default() -> Self {
        Self {
            inner: Arc::new(PauseGateInner {
                closed: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }
}

impl PauseGate {
    /// Waits until the gate is open.
    /// Does not hold any lock - the gate can be closed while caller proceeds.
    pub async fn wait(&self) {
        loop {
            // Register for notification BEFORE checking the flag to avoid race condition.
            // Otherwise: check closed (true) → guard drops & notifies → we call notified() → miss it
            let notified = self.inner.notify.notified();
            if !self.inner.closed.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    /// Closes the gate immediately, blocking new waiters.
    /// Does not wait for current waiters to finish.
    /// Returns a guard that keeps the gate closed until dropped.
    #[must_use = "gate reopens immediately if guard is dropped"]
    pub fn close(&self) -> PauseGateCloseGuard {
        self.inner.closed.store(true, Ordering::Release);
        PauseGateCloseGuard {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[derive(Debug)]
pub struct PauseGateCloseGuard {
    inner: Arc<PauseGateInner>,
}

impl Drop for PauseGateCloseGuard {
    fn drop(&mut self) {
        self.inner.closed.store(false, Ordering::Release);
        self.inner.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[tokio::test]
    async fn waiters_proceed_when_open() {
        let gate = PauseGate::default();
        gate.wait().await;
        // Should not block
    }

    #[tokio::test]
    async fn waiters_block_when_closed() {
        let gate = PauseGate::default();
        let counter = Arc::new(AtomicUsize::new(0));

        let close_guard = gate.close();

        let gate_clone = gate.clone();
        let counter_clone = counter.clone();
        let handle = tokio::spawn(async move {
            gate_clone.wait().await;
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "waiter should be blocked"
        );

        drop(close_guard);

        tokio::time::timeout(Duration::from_millis(100), handle)
            .await
            .expect("task should complete")
            .expect("task should not panic");

        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "waiter should have proceeded"
        );
    }

    #[tokio::test]
    async fn multiple_waiters_proceed_concurrently() {
        let gate = PauseGate::default();
        let counter = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..5 {
            let gate_clone = gate.clone();
            let counter_clone = counter.clone();
            handles.push(tokio::spawn(async move {
                gate_clone.wait().await;
                counter_clone.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }));
        }

        for handle in handles {
            handle.await.expect("task should not panic");
        }

        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn close_does_not_wait_for_current_waiters() {
        let gate = PauseGate::default();
        let in_wait = Arc::new(AtomicBool::new(false));

        let gate_clone = gate.clone();
        let in_wait_clone = in_wait.clone();
        let _handle = tokio::spawn(async move {
            gate_clone.wait().await;
            in_wait_clone.store(true, Ordering::SeqCst);
            // Simulate long work after wait
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        // Wait for spawned task to pass wait()
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            in_wait.load(Ordering::SeqCst),
            "task should have passed wait"
        );

        // close() should return immediately, not wait for the task
        let start = std::time::Instant::now();
        let _guard = gate.close();
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "close should be instant"
        );
    }

    /// Stress test for race condition between wait() and close()/drop().
    /// Without proper synchronization, waiters can miss notifications and block forever.
    #[tokio::test]
    async fn stress_test_no_missed_notifications() {
        let gate = PauseGate::default();
        let iterations = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        // Spawn multiple waiters that spin on wait()
        let mut handles = vec![];
        for _ in 0..4 {
            let gate = gate.clone();
            let iterations = iterations.clone();
            let stop = stop.clone();
            handles.push(tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    gate.wait().await;
                    iterations.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            }));
        }

        // Rapidly close and reopen the gate
        for _ in 0..1000 {
            let _guard = gate.close();
            tokio::task::yield_now().await;
            drop(_guard);
            tokio::task::yield_now().await;
        }

        // Let waiters run a bit more
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.store(true, Ordering::Relaxed);

        // If there's a race condition, waiters will be stuck and this will timeout
        for handle in handles {
            tokio::time::timeout(Duration::from_millis(500), handle)
                .await
                .expect("waiter should not be stuck")
                .expect("waiter should not panic");
        }

        let total = iterations.load(Ordering::Relaxed);
        assert!(
            total > 100,
            "waiters should have completed many iterations, got {}",
            total
        );
    }
}
