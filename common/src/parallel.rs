//! Concurrency-limited parallel iteration utilities.
//!
//! Processes items in consecutive parallel *batches* of `max_concurrent`,
//! bounding peak concurrency — and thus peak memory / open files — when each
//! item is heavy (a large image buffer, an open file). This is a **batched**
//! cap with a barrier between batches, not a sliding window: batch N+1 starts
//! only once batch N has fully finished.
//!
//! That barrier is intentional and cheap for the intended workload, where each
//! `f` is itself CPU-saturating (it uses the global rayon pool internally) or
//! IO-bound — so the few threads that finish a batch early aren't wasted (their
//! cores are taken by neighbours' internal parallelism). A true sliding-window
//! cap would need a non-rayon worker pool (so nested rayon in `f` isn't
//! starved); it isn't worth the complexity here.

use rayon::prelude::*;
#[cfg(test)]
use std::thread;
#[cfg(test)]
use std::time::Duration;

/// Maps `f` over `items`, running at most `max_concurrent` invocations at once.
///
/// Items run in consecutive batches of `max_concurrent` via rayon's `par_iter`,
/// with a barrier between batches (see the module docs). Each `f` keeps full
/// access to the global rayon pool for its own internal parallelism. Results
/// preserve input order.
///
/// # Panics
///
/// Panics if `max_concurrent` is 0.
pub fn par_map_limited<T, R, F>(items: &[T], max_concurrent: usize, f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    assert!(max_concurrent > 0, "max_concurrent must be > 0");

    let mut results = Vec::with_capacity(items.len());
    for chunk in items.chunks(max_concurrent) {
        let chunk_results: Vec<R> = chunk.par_iter().map(&f).collect();
        results.extend(chunk_results);
    }
    results
}

/// Like [`par_map_limited`], but the closure returns `Result<R, E>`.
///
/// Stops at the first batch that contains an error and returns it. Items within
/// the failing batch may still be processed; later batches are not started.
pub fn try_par_map_limited<T, R, E, F>(
    items: &[T],
    max_concurrent: usize,
    f: F,
) -> Result<Vec<R>, E>
where
    T: Sync,
    R: Send,
    E: Send,
    F: Fn(&T) -> Result<R, E> + Sync,
{
    assert!(max_concurrent > 0, "max_concurrent must be > 0");

    let mut results = Vec::with_capacity(items.len());
    for chunk in items.chunks(max_concurrent) {
        let chunk_results: Result<Vec<R>, E> = chunk.par_iter().map(&f).collect();
        results.extend(chunk_results?);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_par_map_limited_basic() {
        let items: Vec<i32> = (0..10).collect();
        let result = par_map_limited(&items, 3, |&x| x * 2);
        assert_eq!(result, vec![0, 2, 4, 6, 8, 10, 12, 14, 16, 18]);
    }

    #[test]
    fn test_par_map_limited_preserves_order() {
        let items: Vec<i32> = (0..100).collect();
        let result = par_map_limited(&items, 4, |&x| x);
        let expected: Vec<i32> = (0..100).collect();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_par_map_limited_single_concurrency() {
        let items: Vec<i32> = (0..5).collect();
        let result = par_map_limited(&items, 1, |&x| x + 1);
        assert_eq!(result, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_par_map_limited_concurrency_exceeds_items() {
        let items: Vec<i32> = (0..3).collect();
        let result = par_map_limited(&items, 100, |&x| x * x);
        assert_eq!(result, vec![0, 1, 4]);
    }

    #[test]
    fn test_par_map_limited_empty() {
        let items: Vec<i32> = vec![];
        let result = par_map_limited(&items, 3, |&x| x);
        assert!(result.is_empty());
    }

    #[test]
    #[should_panic(expected = "max_concurrent must be > 0")]
    fn test_par_map_limited_zero_panics() {
        par_map_limited(&[1, 2, 3], 0, |&x| x);
    }

    #[test]
    fn test_par_map_limited_concurrency_cap() {
        let items: Vec<i32> = (0..20).collect();
        let in_flight = AtomicUsize::new(0);
        let max_observed = AtomicUsize::new(0);

        par_map_limited(&items, 3, |&x| {
            let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_observed.fetch_max(current, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(5));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            x
        });

        let max = max_observed.load(Ordering::SeqCst);
        assert!(max <= 3, "max in-flight was {max}, expected <= 3");
    }

    #[test]
    fn test_try_par_map_limited_ok() {
        let items: Vec<i32> = (0..10).collect();
        let result: Result<Vec<i32>, &str> = try_par_map_limited(&items, 3, |&x| Ok(x * 2));
        assert_eq!(result.unwrap(), vec![0, 2, 4, 6, 8, 10, 12, 14, 16, 18]);
    }

    #[test]
    fn test_try_par_map_limited_err() {
        let items: Vec<i32> = (0..10).collect();
        let result: Result<Vec<i32>, String> = try_par_map_limited(&items, 3, |&x| {
            if x == 5 {
                Err("bad".to_string())
            } else {
                Ok(x)
            }
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_try_par_map_limited_empty() {
        let items: Vec<i32> = vec![];
        let result: Result<Vec<i32>, &str> = try_par_map_limited(&items, 3, |&x| Ok(x));
        assert_eq!(result.unwrap(), Vec::<i32>::new());
    }
}
