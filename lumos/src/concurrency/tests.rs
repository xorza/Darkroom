use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use rayon::ThreadPoolBuilder;

use crate::concurrency::{JobScratchPool, try_par_map_limited, try_par_map_limited_owned};

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
        try_par_map_limited(&items, 3, |_index, value| {
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
fn limited_map_propagates_error_and_stops_taking_work() {
    let started = AtomicUsize::new(0);
    let items: Vec<usize> = (0..1000).collect();

    let result = try_par_map_limited(&items, 3, |_index, value| {
        started.fetch_add(1, Ordering::SeqCst);
        if *value == 0 {
            Err("zero")
        } else {
            Ok(value * 2)
        }
    });

    assert_eq!(result, Err("zero"));
    // Item 0 fails immediately. The other two slots finish whatever they took, and a worker that
    // read the counter just before the failure landed may run one more — a slot's worth of waste,
    // nowhere near the 1000 a run-to-completion would do.
    let ran = started.load(Ordering::SeqCst);
    assert!(ran <= 16, "ran {ran} of 1000 after an immediate failure");
}

#[test]
fn limited_map_accepts_empty_input() {
    let result = try_par_map_limited(&[], 2, |_index, value: &usize| Ok::<_, ()>(*value));
    assert_eq!(result.unwrap(), Vec::<usize>::new());
}

#[test]
#[should_panic(expected = "max_concurrent must be positive")]
fn limited_map_rejects_zero_concurrency() {
    let _ = try_par_map_limited(&[1], 0, |_index, value| Ok::<_, ()>(*value));
}

#[test]
fn owned_map_indexes_by_input_position() {
    // A cell's index is what identifies it once the payload has been taken out, so a slot- or
    // window-relative number here would still pass every order-only assertion.
    let items: Vec<String> = (0..6).map(|i| format!("f{i}")).collect();
    let result = try_par_map_limited_owned(items, 2, |index, value| {
        Ok::<_, ()>(format!("{index}:{value}"))
    });

    assert_eq!(
        result.unwrap(),
        ["0:f0", "1:f1", "2:f2", "3:f3", "4:f4", "5:f5"]
    );
}

#[test]
fn owned_map_propagates_error_and_stops_taking_work() {
    let started = AtomicUsize::new(0);
    let items: Vec<usize> = (0..1000).collect();

    let result = try_par_map_limited_owned(items, 3, |_index, value| {
        started.fetch_add(1, Ordering::SeqCst);
        if value == 0 {
            Err("zero")
        } else {
            Ok(value * 2)
        }
    });

    assert_eq!(result, Err("zero"));
    let ran = started.load(Ordering::SeqCst);
    assert!(ran <= 16, "ran {ran} of 1000 after an immediate failure");
}
