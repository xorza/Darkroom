use std::sync::Arc;
use tokio::sync::RwLock;

/// A synchronization primitive that allows one side to pause waiters.
///
/// The controller can pause/resume, and waiters will block when paused.
/// Multiple waiters can proceed concurrently when not paused.
#[derive(Debug, Clone)]
pub struct PauseGate {
    lock: Arc<RwLock<()>>,
}

impl Default for PauseGate {
    fn default() -> Self {
        Self {
            lock: Arc::new(RwLock::new(())),
        }
    }
}

impl PauseGate {
    /// Waits until the gate is open, then returns a guard.
    /// While the guard is held, the gate cannot be closed.
    pub async fn wait(&self) -> PauseGateGuard<'_> {
        let guard = self.lock.read().await;
        PauseGateGuard { _guard: guard }
    }

    /// Closes the gate, blocking new waiters.
    /// Returns a guard that keeps the gate closed until dropped.
    /// Waits for all current wait() guards to be dropped before returning.
    pub async fn close(&self) -> PauseGateCloseGuard<'_> {
        let guard = self.lock.write().await;
        PauseGateCloseGuard { _guard: guard }
    }
}

#[derive(Debug)]
pub struct PauseGateGuard<'a> {
    _guard: tokio::sync::RwLockReadGuard<'a, ()>,
}

#[derive(Debug)]
pub struct PauseGateCloseGuard<'a> {
    _guard: tokio::sync::RwLockWriteGuard<'a, ()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn waiters_proceed_when_open() {
        let gate = PauseGate::default();

        let _guard = gate.wait().await;
        // Should not block
    }

    #[tokio::test]
    async fn waiters_block_when_closed() {
        let gate = PauseGate::default();
        let counter = Arc::new(AtomicUsize::new(0));

        let _close_guard = gate.close().await;

        let gate_clone = gate.clone();
        let counter_clone = counter.clone();
        let handle = tokio::spawn(async move {
            let _guard = gate_clone.wait().await;
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "waiter should be blocked"
        );

        drop(_close_guard);

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
                let _guard = gate_clone.wait().await;
                counter_clone.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }));
        }

        for handle in handles {
            handle.await.expect("task should not panic");
        }

        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }
}
