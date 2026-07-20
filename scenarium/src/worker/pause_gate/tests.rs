use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::time::timeout;

use crate::worker::pause_gate::PauseGate;

async fn assert_closed(gate: &PauseGate) {
    assert!(
        timeout(Duration::from_millis(10), gate.wait())
            .await
            .is_err(),
        "closed gate admitted a waiter"
    );
}

#[tokio::test]
async fn wait_returns_immediately_when_open() {
    timeout(Duration::from_millis(50), PauseGate::default().wait())
        .await
        .expect("open gate blocked");
}

#[tokio::test]
async fn waiters_block_until_the_gate_reopens() {
    let gate = PauseGate::default();
    let guard = gate.close();
    let completed = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    for _ in 0..5 {
        let gate = gate.clone();
        let completed = Arc::clone(&completed);
        handles.push(tokio::spawn(async move {
            gate.wait().await;
            completed.fetch_add(1, Ordering::Relaxed);
        }));
    }

    tokio::task::yield_now().await;
    assert_eq!(completed.load(Ordering::Relaxed), 0);
    drop(guard);

    for handle in handles {
        timeout(Duration::from_millis(100), handle)
            .await
            .expect("reopened gate did not wake every waiter")
            .expect("waiter task panicked");
    }
    assert_eq!(completed.load(Ordering::Relaxed), 5);
}

#[tokio::test]
async fn overlapping_guards_from_clones_reopen_only_after_the_last_drop() {
    for drop_original_first in [true, false] {
        let gate = PauseGate::default();
        let clone = gate.clone();
        let mut original_guard = Some(gate.close());
        let mut clone_guard = Some(clone.close());

        assert_closed(&gate).await;
        if drop_original_first {
            drop(original_guard.take());
        } else {
            drop(clone_guard.take());
        }
        assert_closed(&gate).await;

        drop(original_guard.take());
        drop(clone_guard.take());
        timeout(Duration::from_millis(50), gate.wait())
            .await
            .expect("last guard did not reopen the gate");
    }
}

#[tokio::test]
async fn close_does_not_revoke_a_waiter_that_already_passed() {
    let gate = PauseGate::default();
    let passed = Arc::new(AtomicBool::new(false));
    let release = Arc::new(tokio::sync::Notify::new());
    let handle = tokio::spawn({
        let gate = gate.clone();
        let passed = Arc::clone(&passed);
        let release = Arc::clone(&release);
        async move {
            gate.wait().await;
            passed.store(true, Ordering::Release);
            release.notified().await;
        }
    });

    while !passed.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
    }
    let _guard = gate.close();
    release.notify_one();
    timeout(Duration::from_millis(50), handle)
        .await
        .expect("closing waited for work beyond the gate")
        .expect("waiter task panicked");
}

#[tokio::test]
async fn repeated_close_and_reopen_never_strands_waiters() {
    let gate = PauseGate::default();
    let completed = Arc::new(AtomicUsize::new(0));

    for _ in 0..100 {
        let guard = gate.close();
        let handle = tokio::spawn({
            let gate = gate.clone();
            let completed = Arc::clone(&completed);
            async move {
                gate.wait().await;
                completed.fetch_add(1, Ordering::Relaxed);
            }
        });
        tokio::task::yield_now().await;
        drop(guard);
        timeout(Duration::from_millis(100), handle)
            .await
            .expect("waiter missed the reopen notification")
            .expect("waiter task panicked");
    }

    assert_eq!(completed.load(Ordering::Relaxed), 100);
}
