use std::hint;
use std::mem;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::{PerWorker, WorkerPool, par};

#[test]
fn slots_preserve_order_and_reach_the_exact_cap() {
    let barrier = Barrier::new(3);
    let in_flight = AtomicUsize::new(0);
    let max_observed = AtomicUsize::new(0);

    let result = par::slots(6, &mut [(); 3], |(), idx| {
        let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        max_observed.fetch_max(cur, Ordering::SeqCst);
        barrier.wait();
        in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok::<_, ()>(idx * 2)
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

    let result = par::slots(1000, &mut [(); SLOTS], |(), idx| {
        started.fetch_add(1, Ordering::SeqCst);
        if idx == 0 {
            failed.store(true, Ordering::SeqCst);
            return Err("zero");
        }
        wait_for_failure(&failed);
        Ok(idx)
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
    let result = par::slots(4, &mut [(); 2], |(), idx| match idx {
        0 => {
            wait_for_failure(&failed);
            Err(0)
        }
        1 => {
            failed.store(true, Ordering::SeqCst);
            Err(1)
        }
        _ => Ok(()),
    });
    assert_eq!(result, Err(0));
}

#[test]
fn a_slot_carries_its_state_between_the_indices_it_takes_and_each_job_runs_parallel_work() {
    let mut counts = [0_usize; 2];
    let result = par::slots(6, &mut counts, |count, idx| {
        *count += 1;
        // A job is no loop's body, so it starts loops of its own: 0 + 1 + .. + idx.
        Ok::<_, ()>(par::into_iter(0..=idx).sum::<usize>())
    });
    assert_eq!(result.unwrap(), [0, 1, 3, 6, 10, 15]);
    assert_eq!(counts.iter().sum::<usize>(), 6);
}

/// A job gets the item at its own index, moved in, and the slot keeps what the job before it left
/// there: with two slots over six items, at least four jobs find a predecessor's item waiting.
#[test]
fn owned_slots_hand_each_job_its_item_and_carry_the_slot_between_items() {
    const SLOTS: usize = 2;
    let items: Vec<String> = (0..6).map(|i| format!("f{i}")).collect();
    let mut slots = vec![String::new(); SLOTS];
    let result = par::slots_owned(items, &mut slots, |slot, idx, value| {
        let carried = mem::replace(slot, value.clone());
        Ok::<_, ()>(format!("{idx}:{value}:{carried}"))
    })
    .unwrap();
    let heads: Vec<&str> = result.iter().map(|r| &r[..4]).collect();
    assert_eq!(heads, ["0:f0", "1:f1", "2:f2", "3:f3", "4:f4", "5:f5"]);
    let carried = result.iter().filter(|r| !r.ends_with(':')).count();
    assert!(
        carried >= 6 - SLOTS,
        "slots carried {carried} of 6: {result:?}"
    );
}

#[test]
fn slots_accept_no_index_and_leave_the_slots_past_the_last_untouched() {
    let result = par::slots(0, &mut [()], |(), idx| Ok::<_, ()>(idx));
    assert_eq!(result.unwrap(), Vec::<usize>::new());
    let mut used = [false; 4];
    par::slots(2, &mut used, |used, _| {
        *used = true;
        Ok::<_, ()>(())
    })
    .unwrap();
    // One slot can take both indices before the other starts, so only the first two may be used.
    assert!(used[..2].contains(&true));
    assert_eq!(used[2..], [false, false]);
}

#[test]
#[should_panic(expected = "a bounded map needs at least one slot")]
fn slots_refuse_zero_slots() {
    let no_slots: &mut [()] = &mut [];
    let _refused = par::slots(1, no_slots, |(), idx| Ok::<_, ()>(idx));
}

#[test]
#[should_panic(expected = "a parallel loop's body starts no parallel work")]
fn a_loop_started_in_a_body_panics() {
    par::into_iter(0..4).for_each(|_| {
        par::into_iter(0..2).for_each(|_| {});
    });
}

/// The check sits at the entry, so a body behind any adapter is held to it: here one a zip of
/// two chunked slices runs, lumos's most common shape.
#[test]
#[should_panic(expected = "a parallel loop's body starts no parallel work")]
fn a_loop_started_in_a_zipped_body_panics() {
    let (mut out, input) = (vec![0_u32; 64], vec![1_u32; 64]);
    par::chunks_mut(&mut out, 8)
        .zip(par::chunks(&input, 8))
        .for_each(|(out, input)| {
            let _sum: u32 = par::iter(input).sum();
            out[0] = 1;
        });
}

#[test]
#[should_panic(expected = "a parallel loop's body starts no parallel work")]
fn slots_started_in_a_body_panic() {
    par::into_iter(0..2).for_each(|_| {
        let _ = par::slots(1, &mut [()], |(), idx| Ok::<_, ()>(idx));
    });
}

/// Two items split, and a split loop runs both halves on workers.
#[test]
#[should_panic(expected = "a worker pool runs from outside every pool")]
fn a_pool_refuses_to_run_from_a_worker() {
    let inner = WorkerPool::new("par-inner", 2);
    par::into_iter(0..2).for_each(|_| inner.run(|| ()));
}

#[test]
fn each_entry_point_runs_its_rayon_counterpart_and_a_pool_counts_its_own_workers() {
    WorkerPool::new("par-test", 3).run(|| {
        assert_eq!(par::threads(), 3);
        let mut values: Vec<u32> = (1..=10).collect();
        assert_eq!(par::iter(&values).sum::<u32>(), 55);
        par::iter_mut(&mut values).for_each(|value| *value *= 2);
        assert_eq!(values, [2, 4, 6, 8, 10, 12, 14, 16, 18, 20]);
        // Chunks of 4 over 10: 4 + 4 + 2, and the exact ones leave the 2 out.
        let lens: Vec<usize> = par::chunks(&values, 4).map(<[u32]>::len).collect();
        assert_eq!(lens, [4, 4, 2]);
        assert_eq!(par::chunks_exact(&values, 4).count(), 2);
        par::chunks_mut(&mut values, 4).for_each(|chunk| chunk[0] = 0);
        par::chunks_exact_mut(&mut values, 4).for_each(|chunk| chunk[1] = 1);
        assert_eq!(values, [0, 1, 6, 8, 0, 1, 14, 16, 0, 20]);
        let drained: Vec<u32> = par::drain(&mut values, 8..).collect();
        assert_eq!((drained, values.len()), (vec![0, 20], 8));
        par::extend(&mut values, vec![7_u32, 9]);
        assert_eq!(values[8..], [7, 9]);
    });
    assert_eq!(par::threads(), rayon::current_num_threads());
}

#[test]
fn per_worker_makes_one_slot_for_each_worker_and_each_body_takes_its_own() {
    const WORKERS: usize = 4;
    let pool = WorkerPool::new("par-test", WORKERS);
    let made = AtomicUsize::new(0);
    let mut seen = pool.run(|| {
        PerWorker::new(|| {
            made.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        })
    });
    for _ in 0..2 {
        pool.run(|| par::into_iter(0..1000_usize).for_each(|idx| seen.get().push(idx)));
    }
    // Made once per worker and once outside the pool, not per split, and kept across both runs.
    assert_eq!(
        (made.load(Ordering::SeqCst), seen.slot_count()),
        (WORKERS + 1, WORKERS + 1)
    );
    let mut all: Vec<usize> = seen.iter_mut().flat_map(|slot| slot.drain(..)).collect();
    all.sort_unstable();
    let twice: Vec<usize> = (0..1000).flat_map(|idx| [idx, idx]).collect();
    assert_eq!(all, twice);
}

/// One worker's slots, made on a pool of one.
fn one_slot() -> PerWorker<u32> {
    WorkerPool::new("par-test", 1).run(|| PerWorker::new(|| 0))
}

#[test]
#[should_panic(expected = "slot is taken")]
fn a_slot_taken_twice_panics() {
    let scratch = one_slot();
    WorkerPool::new("par-test", 1).run(|| {
        par::into_iter(0..1).for_each(|_| {
            let _held = scratch.get();
            let _again = scratch.get();
        });
    });
}

/// A loop rayon does not split runs its body on the caller's own thread: that body, and any
/// caller outside the pool, take the slot outside it.
#[test]
fn a_body_on_its_caller_s_thread_takes_the_slot_outside_the_pool() {
    let mut scratch = one_slot();
    par::into_iter(0..1).for_each(|_| *scratch.get() += 1);
    *scratch.get() += 10;
    let slots: Vec<u32> = scratch.iter_mut().map(|slot| *slot).collect();
    assert_eq!(slots, [0, 11]);
}

#[test]
#[should_panic(expected = "made for a smaller pool")]
fn a_slot_on_a_larger_pool_panics() {
    let scratch = one_slot();
    // Both bodies wait for each other before taking a slot, so they run on both workers.
    let both = Barrier::new(2);
    WorkerPool::new("par-test", 2).run(|| {
        par::into_iter(0..2).with_min_len(1).for_each(|_| {
            both.wait();
            *scratch.get() += 1;
        });
    });
}
