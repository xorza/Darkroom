//! Concurrency-limited parallel iteration utilities.
//!
//! Wraps rayon's `par_iter` to process items in parallel while limiting
//! the number of items in flight at once (e.g. to cap memory or IO pressure).

use rayon::prelude::*;

/// Maps `f` over `items` in parallel, with at most `max_concurrent` items in flight.
///
/// Semantically equivalent to `items.par_iter().map(f).collect()`, but processes
/// items in chunks of `max_concurrent` to limit resource usage.
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
/// Stops at the first chunk that contains an error and returns it.
/// Items within the failing chunk may still be processed in parallel.
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
            std::thread::sleep(std::time::Duration::from_millis(5));
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
