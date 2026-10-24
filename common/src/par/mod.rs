//! Parallel work on rayon's global pool.
//!
//! A worker that blocks in a rayon call runs other queued jobs on its own stack, so a parallel
//! loop started inside another loop's body puts that body's state under unrelated work: per-worker
//! scratch stops being exclusive and every per-worker memory figure is wrong. Rayon has no way to
//! isolate the inner work (oneTBB's `this_task_arena::isolate` is the reference answer), so the
//! rule is that a loop body never starts parallel work.
//!
//! Each entry point checks the rule. Top-level code never runs on a pool worker: callers are the
//! application's own threads, and [`slots`] runs its jobs on plain threads, as Polars moves
//! blocking work off its pool. So a caller on a worker is a loop's body, and the entry point
//! panics. A loop rayon does not split, one of one item, runs its body on the caller's own thread,
//! where a parallel loop passes: that thread is no worker, so it waits on the loop without taking
//! other jobs. The entry points return rayon's own iterators, which every adapter after them
//! keeps.

pub(super) mod per_worker;
#[cfg(feature = "internals")]
pub(super) mod worker_pool;

use std::ops::RangeBounds;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::iter::{
    IntoParallelIterator, IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelDrainRange,
    ParallelExtend,
};
use rayon::slice::{
    Chunks, ChunksExact, ChunksExactMut, ChunksMut, Iter, IterMut, ParallelSlice, ParallelSliceMut,
};
use rayon::vec::Drain;

use crate::thread;

/// The workers a parallel loop started here spreads over, the count a per-worker memory figure
/// multiplies.
pub fn threads() -> usize {
    rayon::current_num_threads()
}

/// `items.par_iter()`.
///
/// # Panics
/// In a parallel loop's body, as every entry point here does.
pub fn iter<T: Sync>(items: &[T]) -> Iter<'_, T> {
    enter();
    items.par_iter()
}

/// `items.par_iter_mut()`.
pub fn iter_mut<T: Send>(items: &mut [T]) -> IterMut<'_, T> {
    enter();
    items.par_iter_mut()
}

/// `items.into_par_iter()`.
pub fn into_iter<I: IntoParallelIterator>(items: I) -> I::Iter {
    enter();
    items.into_par_iter()
}

/// `items.par_chunks(size)`.
pub fn chunks<T: Sync>(items: &[T], size: usize) -> Chunks<'_, T> {
    enter();
    items.par_chunks(size)
}

/// `items.par_chunks_exact(size)`.
pub fn chunks_exact<T: Sync>(items: &[T], size: usize) -> ChunksExact<'_, T> {
    enter();
    items.par_chunks_exact(size)
}

/// `items.par_chunks_mut(size)`.
pub fn chunks_mut<T: Send>(items: &mut [T], size: usize) -> ChunksMut<'_, T> {
    enter();
    items.par_chunks_mut(size)
}

/// `items.par_chunks_exact_mut(size)`.
pub fn chunks_exact_mut<T: Send>(items: &mut [T], size: usize) -> ChunksExactMut<'_, T> {
    enter();
    items.par_chunks_exact_mut(size)
}

/// `items.par_drain(range)`.
pub fn drain<T: Send>(items: &mut Vec<T>, range: impl RangeBounds<usize>) -> Drain<'_, T> {
    enter();
    items.par_drain(range)
}

/// `target.par_extend(items)`.
pub fn extend<T: Send>(target: &mut Vec<T>, items: impl IntoParallelIterator<Item = T>) {
    enter();
    target.par_extend(items);
}

/// Runs `job` over `0..len` with one slot bound to each in-flight index, at most
/// `slots.len()` at a time, and splices the results back into index order.
///
/// Each slot is a plain thread of its own, not a pool task, so a job is a whole stage of work (a
/// frame's decode, a frame's warp) whose loops run in parallel too: nothing nests on a worker's
/// stack, and a slot finishes when its own job does. Each slot takes the next index the moment it
/// frees up: batching the indices instead would make every batch wait on its slowest member. A
/// failure stops the slots from taking indices past it; those already running finish. Of several
/// failures the one at the lowest index is returned, the failure a sequential map would return.
///
/// # Panics
/// With no slot, or in a parallel loop's body.
pub fn slots<S, R, E>(
    len: usize,
    slots: &mut [S],
    job: impl Fn(&mut S, usize) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E>
where
    S: Send,
    R: Send,
    E: Send,
{
    assert!(!slots.is_empty(), "a bounded map needs at least one slot");
    enter();
    let next = AtomicUsize::new(0);
    let lowest_failure = AtomicUsize::new(usize::MAX);
    let mut outcomes: Vec<SlotOutcome<R, E>> =
        slots.iter().map(|_| SlotOutcome::default()).collect();
    thread::scope(|scope| {
        // A slot past the last index would find none: no thread for it.
        let busy = slots.iter_mut().zip(&mut outcomes).take(len);
        for (idx, (slot, outcome)) in busy.enumerate() {
            let (next, lowest_failure, job) = (&next, &lowest_failure, &job);
            scope.spawn(format!("slot-{idx}"), move || {
                loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    if idx >= len || idx > lowest_failure.load(Ordering::Acquire) {
                        break;
                    }
                    match job(slot, idx) {
                        Ok(value) => outcome.values.push(Indexed { idx, value }),
                        Err(error) => {
                            lowest_failure.fetch_min(idx, Ordering::AcqRel);
                            outcome.failure = Some(Indexed { idx, value: error });
                            return;
                        }
                    }
                }
            });
        }
    });

    let mut ordered: Vec<Option<R>> = (0..len).map(|_| None).collect();
    let mut first_failure: Option<Indexed<E>> = None;
    for outcome in outcomes {
        for Indexed { idx, value } in outcome.values {
            ordered[idx] = Some(value);
        }
        if let Some(failure) = outcome.failure
            && first_failure
                .as_ref()
                .is_none_or(|first| failure.idx < first.idx)
        {
            first_failure = Some(failure);
        }
    }
    if let Some(failure) = first_failure {
        return Err(failure.value);
    }
    Ok(ordered
        .into_iter()
        .map(|value| value.expect("each index below len is claimed by exactly one slot"))
        .collect())
}

/// [`slots`] with each of `items` moved into its job, so a caller frees each input as soon as its
/// output exists rather than holding every input and every output at once. What stays resident of
/// an item once its job took it is one lock and an empty `Option`.
///
/// # Panics
/// As [`slots`] does.
pub fn slots_owned<T, S, R, E>(
    items: Vec<T>,
    slots: &mut [S],
    job: impl Fn(&mut S, usize, T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E>
where
    T: Send,
    S: Send,
    R: Send,
    E: Send,
{
    let cells: Vec<Mutex<Option<T>>> = items
        .into_iter()
        .map(|item| Mutex::new(Some(item)))
        .collect();
    self::slots(cells.len(), slots, |slot, idx| {
        let item = cells[idx]
            .lock()
            .expect("no holder of this lock panicked")
            .take()
            .expect("each index is claimed by exactly one slot");
        job(slot, idx, item)
    })
}

/// The check every entry point here runs, for a wrapper around another crate's function that
/// starts parallel work of its own and cannot call this module.
///
/// # Panics
/// On a pool worker, where only a parallel loop's body runs.
pub fn enter() {
    assert!(
        outside_body(),
        "a parallel loop's body starts no parallel work: a worker waiting on it would run other \
         jobs on its stack"
    );
}

fn outside_body() -> bool {
    #[cfg(feature = "internals")]
    if worker_pool::WorkerPool::is_top_level() {
        return true;
    }
    rayon::current_thread_index().is_none()
}

/// What one slot of [`slots`] finished with: the values of the indices it took, and the failure
/// that stopped it, if one did.
#[derive(Debug)]
struct SlotOutcome<R, E> {
    values: Vec<Indexed<R>>,
    failure: Option<Indexed<E>>,
}

impl<R, E> Default for SlotOutcome<R, E> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            failure: None,
        }
    }
}

/// A job's value or failure, and the index it ran for.
#[derive(Debug)]
struct Indexed<T> {
    idx: usize,
    value: T,
}

#[cfg(test)]
mod tests;
