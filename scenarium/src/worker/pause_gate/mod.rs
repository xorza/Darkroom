use tokio::sync::watch;

/// A synchronization primitive that allows one side to pause waiters.
///
/// The controller can pause/resume, and waiters will block when paused.
/// Multiple waiters can proceed concurrently when not paused.
/// Closing the gate is instant and does not wait for current waiters.
#[derive(Debug, Clone)]
pub(crate) struct PauseGate {
    close_count: watch::Sender<usize>,
}

impl Default for PauseGate {
    fn default() -> Self {
        Self {
            close_count: watch::Sender::new(0),
        }
    }
}

impl PauseGate {
    /// Waits until the gate is open.
    /// Does not hold any lock - the gate can be closed while caller proceeds.
    pub(crate) async fn wait(&self) {
        self.close_count
            .subscribe()
            .wait_for(|close_count| *close_count == 0)
            .await
            .expect("PauseGate retains its sender");
    }

    /// Closes the gate immediately, blocking new waiters.
    /// Does not wait for current waiters to finish.
    /// Returns a guard that keeps the gate closed until dropped.
    #[must_use = "gate reopens immediately if guard is dropped"]
    pub(crate) fn close(&self) -> PauseGateCloseGuard {
        let _ = self.close_count.send_if_modified(|close_count| {
            *close_count = close_count
                .checked_add(1)
                .expect("PauseGate close count overflow");
            // Existing waiters only need notification when the last guard reopens the gate.
            false
        });
        PauseGateCloseGuard {
            close_count: self.close_count.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PauseGateCloseGuard {
    close_count: watch::Sender<usize>,
}

impl Drop for PauseGateCloseGuard {
    fn drop(&mut self) {
        let _ = self.close_count.send_if_modified(|close_count| {
            *close_count = close_count
                .checked_sub(1)
                .expect("PauseGate close guard underflow");
            *close_count == 0
        });
    }
}

#[cfg(test)]
mod tests;
