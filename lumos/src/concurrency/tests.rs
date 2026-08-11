use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use rayon::ThreadPoolBuilder;

use crate::concurrency::{JobScratchPool, try_par_map_bounded_owned, try_par_map_limited};

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

/// Hold a job until `failed` is set, so the workers already in flight cannot race ahead through
/// the cheap remainder before the failure is visible.
///
/// The wait always ends: indices are handed out by one `fetch_add`, so index 0 — the one that
/// fails — is claimed before any other, and whichever worker holds it is running. The iteration
/// cap only stops a regression from hanging the suite instead of failing it.
fn wait_for_failure(failed: &AtomicBool) {
    for _ in 0..1_000_000_000u64 {
        if failed.load(Ordering::SeqCst) {
            return;
        }
        std::hint::spin_loop();
    }
}

#[test]
fn limited_map_propagates_error_and_stops_taking_work() {
    const SLOTS: usize = 3;
    let started = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let items: Vec<usize> = (0..1000).collect();

    let result = try_par_map_limited(&items, SLOTS, |_index, value| {
        started.fetch_add(1, Ordering::SeqCst);
        if *value == 0 {
            failed.store(true, Ordering::SeqCst);
            return Err("zero");
        }
        wait_for_failure(&failed);
        Ok(value * 2)
    });

    assert_eq!(result, Err("zero"));
    // Every worker is holding its first item when the failure lands, so each can only finish that
    // one and then find the flag set. At most one item per slot ran; the 997 after them did not.
    let ran = started.load(Ordering::SeqCst);
    assert!(
        ran <= SLOTS,
        "ran {ran} of 1000 with {SLOTS} slots after an immediate failure"
    );
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
fn owned_map_indexes_by_input_position_and_carries_a_slot_between_items() {
    // A cell's index is what identifies it once the payload has been taken out, so a slot- or
    // window-relative number here would still pass every order-only assertion.
    let items: Vec<String> = (0..6).map(|i| format!("f{i}")).collect();
    let mut slots = vec![String::new(); 2];
    let result = try_par_map_bounded_owned(items, &mut slots, |slot, index, value| {
        // The slot arrives holding whatever this worker last left in it, which is the whole point:
        // it outlives the item, unlike anything the closure could build per call.
        let carried = std::mem::replace(slot, value.clone());
        Ok::<_, ()>(format!("{index}:{value}:{carried}"))
    });

    let result = result.unwrap();
    assert_eq!(
        result.iter().map(|r| &r[..4]).collect::<Vec<_>>(),
        ["0:f0", "1:f1", "2:f2", "3:f3", "4:f4", "5:f5"]
    );
    // Two workers over six items: four of them must have found a predecessor's value waiting.
    assert_eq!(
        result.iter().filter(|r| !r.ends_with(':')).count(),
        4,
        "slots did not carry between items: {result:?}"
    );
}

#[test]
fn owned_map_propagates_error_and_stops_taking_work() {
    const SLOTS: usize = 3;
    let started = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let items: Vec<usize> = (0..1000).collect();

    let mut slots = vec![(); SLOTS];
    let result = try_par_map_bounded_owned(items, &mut slots, |(), _index, value| {
        started.fetch_add(1, Ordering::SeqCst);
        if value == 0 {
            failed.store(true, Ordering::SeqCst);
            return Err("zero");
        }
        wait_for_failure(&failed);
        Ok(value * 2)
    });

    assert_eq!(result, Err("zero"));
    // Held the same way as the borrowed variant above, so the bound is the slot count.
    let ran = started.load(Ordering::SeqCst);
    assert!(
        ran <= SLOTS,
        "ran {ran} of 1000 with {SLOTS} slots after an immediate failure"
    );
}
