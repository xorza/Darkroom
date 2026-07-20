use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use rayon::ThreadPoolBuilder;

use crate::concurrency::{JobScratchPool, try_par_map_limited};

#[test]
fn job_scratch_leases_are_exclusive_and_reused() {
    let pool = JobScratchPool::<Box<u8>>::default();
    let mut first = pool.acquire();
    **first = 1;
    let mut second = pool.acquire();
    **second = 2;
    let first_address = (&raw const **first).addr();
    let second_address = (&raw const **second).addr();
    assert_ne!(first_address, second_address);

    **first = 3;
    drop(first);
    let reused = pool.acquire();
    assert_eq!((&raw const **reused).addr(), first_address);
    assert_eq!(**reused, 3);
}

#[test]
fn limited_map_preserves_order_and_reaches_the_exact_cap() {
    let pool = ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let in_flight = AtomicUsize::new(0);
    let max_observed = AtomicUsize::new(0);
    let items: Vec<usize> = (0..6).collect();

    let result = pool.install(|| {
        try_par_map_limited(&items, 3, |value| {
            let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_observed.fetch_max(current, Ordering::SeqCst);
            barrier.wait();
            in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok::<_, ()>(value * 2)
        })
    });

    assert_eq!(result.unwrap(), vec![0, 2, 4, 6, 8, 10]);
    assert_eq!(max_observed.load(Ordering::SeqCst), 3);
    assert_eq!(in_flight.load(Ordering::SeqCst), 0);
}

#[test]
fn limited_map_propagates_error_without_starting_later_batches() {
    let started = AtomicUsize::new(0);
    let items: Vec<usize> = (0..9).collect();

    let result = try_par_map_limited(&items, 3, |value| {
        started.fetch_or(1 << value, Ordering::SeqCst);
        if *value == 4 {
            Err("four")
        } else {
            Ok(value * 2)
        }
    });

    assert_eq!(result, Err("four"));
    assert_eq!(started.load(Ordering::SeqCst) & 0b111_000_000, 0);
}

#[test]
fn limited_map_accepts_empty_input() {
    let result = try_par_map_limited(&[], 2, |value: &usize| Ok::<_, ()>(*value));
    assert_eq!(result.unwrap(), Vec::<usize>::new());
}

#[test]
#[should_panic(expected = "max_concurrent must be positive")]
fn limited_map_rejects_zero_concurrency() {
    let _ = try_par_map_limited(&[1], 0, |value| Ok::<_, ()>(*value));
}
