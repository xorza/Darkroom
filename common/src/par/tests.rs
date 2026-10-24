use std::hint;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use rayon::prelude::*;
use static_assertions::assert_not_impl_any;

use crate::{Par, PerWorker, WorkerPool};

// A body rayon runs must be `Send + Sync`, so a token that is neither cannot reach one.
assert_not_impl_any!(Par<'static>: Send, Sync);

#[test]
fn slots_preserve_order_and_reach_the_exact_cap() {
    let pool = WorkerPool::new("par-test", 4);
    let barrier = Arc::new(Barrier::new(3));
    let in_flight = AtomicUsize::new(0);
    let max_observed = AtomicUsize::new(0);

    let result = pool.run(|par| {
        par.slots(6, &mut [(); 3], |(), idx, _| {
            let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_observed.fetch_max(cur, Ordering::SeqCst);
            barrier.wait();
            in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok::<_, ()>(idx * 2)
        })
    });

    assert_eq!(result.unwrap(), [0, 2, 4, 6, 8, 10]);
    assert_eq!(max_observed.load(Ordering::SeqCst), 3);
    assert_eq!(in_flight.load(Ordering::SeqCst), 0);
}

/// Holds a job until `failed` is set, so the slots already in flight cannot race ahead through
/// the cheap remainder before the failure is visible. Index 0, the one that fails, is claimed
/// before any other, so the wait ends; the cap only turns a regression into a failure, not a hang.
fn wait_for_failure(failed: &AtomicBool) {
    for _ in 0..1_000_000_000_u64 {
        if failed.load(Ordering::SeqCst) {
            return;
        }
        hint::spin_loop();
    }
}

#[test]
fn a_failure_stops_the_slots_taking_work() {
    const SLOTS: usize = 3;
    let started = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);

    let result = WorkerPool::new("par-test", 4).run(|par| {
        par.slots(1000, &mut [(); SLOTS], |(), idx, _| {
            started.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                failed.store(true, Ordering::SeqCst);
                return Err("zero");
            }
            wait_for_failure(&failed);
            Ok(idx)
        })
    });

    assert_eq!(result, Err("zero"));
    // Each slot holds its first index when the failure lands, so it finishes that one and finds
    // the flag set: at most one index per slot ran.
    let ran = started.load(Ordering::SeqCst);
    assert!(ran <= SLOTS, "ran {ran} of 1000 with {SLOTS} slots");
}

/// Index 1 fails at once, index 0 waits for that and fails too: the map returns index 0's error,
/// as a sequential map would.
#[test]
fn the_lowest_failing_index_is_reported() {
    let failed = AtomicBool::new(false);
    let result = WorkerPool::new("par-test", 2).run(|par| {
        par.slots(4, &mut [(); 2], |(), idx, _| match idx {
            0 => {
                wait_for_failure(&failed);
                Err(0)
            }
            1 => {
                failed.store(true, Ordering::SeqCst);
                Err(1)
            }
            _ => Ok(()),
        })
    });
    assert_eq!(result, Err(0));
}

#[test]
fn a_slot_carries_its_state_between_the_indices_it_takes_and_each_job_runs_parallel_work() {
    let pool = WorkerPool::new("par-test", 4);
    let mut counts = [0_usize; 2];
    let result = pool.run(|par| {
        par.slots(6, &mut counts, |count, idx, own| {
            *count += 1;
            // A job's own `Par` starts loops of its own: 0 + 1 + .. + idx.
            Ok::<_, ()>(own.into_iter(0..=idx).sum::<usize>())
        })
    });
    assert_eq!(result.unwrap(), [0, 1, 3, 6, 10, 15]);
    assert_eq!(counts.iter().sum::<usize>(), 6);
}

#[test]
fn slots_accept_no_index() {
    let result = WorkerPool::new("par-test", 1)
        .run(|par| par.slots(0, &mut [()], |(), idx, _| Ok::<_, ()>(idx)));
    assert_eq!(result.unwrap(), Vec::<usize>::new());
}

#[test]
#[should_panic(expected = "a bounded map needs at least one slot")]
fn slots_refuse_zero_slots() {
    WorkerPool::new("par-test", 1).run(|par| {
        let no_slots: &mut [()] = &mut [];
        let _refused = par.slots(1, no_slots, |(), idx, _| Ok::<_, ()>(idx));
    });
}

#[test]
#[should_panic(expected = "a worker pool runs from outside every pool")]
fn a_pool_refuses_to_run_from_a_worker() {
    let (outer, inner) = (
        WorkerPool::new("par-outer", 2),
        WorkerPool::new("par-inner", 2),
    );
    outer.run(|par| {
        par.into_iter(0..1).for_each(|_| inner.run(|_| ()));
    });
}

#[test]
fn par_counts_its_pool_s_workers_and_each_entry_point_runs_its_rayon_counterpart() {
    let pool = WorkerPool::new("par-test", 3);
    assert_eq!(pool.threads(), 3);
    pool.run(|par| {
        assert_eq!(par.threads(), 3);
        let mut values: Vec<u32> = (1..=10).collect();
        assert_eq!(par.iter(&values).sum::<u32>(), 55);
        par.iter_mut(&mut values).for_each(|value| *value *= 2);
        assert_eq!(values, [2, 4, 6, 8, 10, 12, 14, 16, 18, 20]);
        // Chunks of 4 over 10: 4 + 4 + 2, and the exact ones leave the 2 out.
        assert_eq!(
            par.chunks(&values, 4).map(<[u32]>::len).collect::<Vec<_>>(),
            [4, 4, 2]
        );
        assert_eq!(par.chunks_exact(&values, 4).count(), 2);
        par.chunks_mut(&mut values, 4)
            .for_each(|chunk| chunk[0] = 0);
        par.chunks_exact_mut(&mut values, 4)
            .for_each(|chunk| chunk[1] = 1);
        assert_eq!(values, [0, 1, 6, 8, 0, 1, 14, 16, 0, 20]);
        let drained: Vec<u32> = par.drain(&mut values, 8..).collect();
        assert_eq!((drained, values.len()), (vec![0, 20], 8));
        par.extend(&mut values, par.into_iter(vec![7_u32, 9]));
        assert_eq!(values[8..], [7, 9]);
    });
}

#[test]
fn per_worker_makes_one_slot_for_each_worker_and_each_body_takes_its_own() {
    const WORKERS: usize = 4;
    let pool = WorkerPool::new("par-test", WORKERS);
    let made = AtomicUsize::new(0);
    let mut seen = pool.run(|par| {
        PerWorker::new(par, || {
            made.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        })
    });
    for _ in 0..2 {
        pool.run(|par| {
            par.into_iter(0..1000_usize)
                .for_each(|idx| seen.get().push(idx));
        });
    }
    // Made once per worker, not per split, and kept across both runs.
    assert_eq!(
        (made.load(Ordering::SeqCst), seen.workers()),
        (WORKERS, WORKERS)
    );
    let mut all: Vec<usize> = seen.iter_mut().flat_map(|slot| slot.drain(..)).collect();
    all.sort_unstable();
    let twice: Vec<usize> = (0..1000).flat_map(|idx| [idx, idx]).collect();
    assert_eq!(all, twice);
}

/// One worker's slots, made on a pool of one.
fn one_slot() -> PerWorker<u32> {
    WorkerPool::new("par-test", 1).run(|par| PerWorker::new(par, || 0))
}

#[test]
#[should_panic(expected = "slot is taken")]
fn a_slot_taken_twice_panics() {
    let scratch = one_slot();
    WorkerPool::new("par-test", 1).run(|par| {
        par.into_iter(0..1).for_each(|_| {
            let _held = scratch.get();
            let _again = scratch.get();
        });
    });
}

#[test]
#[should_panic(expected = "a worker's slot is taken on a worker")]
fn a_slot_off_a_worker_panics() {
    let scratch = one_slot();
    let _slot = scratch.get();
}

#[test]
#[should_panic(expected = "made for a smaller pool")]
fn a_slot_on_a_larger_pool_panics() {
    let scratch = one_slot();
    // Both bodies wait for each other before taking a slot, so they run on both workers.
    let both = Barrier::new(2);
    WorkerPool::new("par-test", 2).run(|par| {
        par.into_iter(0..2).with_min_len(1).for_each(|_| {
            both.wait();
            *scratch.get() += 1;
        });
    });
}
