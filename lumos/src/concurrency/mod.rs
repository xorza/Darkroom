//! Concurrency helpers for Rayon work and reusable per-job resources.

pub(crate) mod job_scratch_pool;
pub(crate) mod unsafe_send_ptr;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What one slot of a bounded map finished with: the values of the indices it took, and the
/// failure that stopped it, if one did.
#[derive(Debug)]
struct SlotOutcome<R, E> {
    values: Vec<(usize, R)>,
    failure: Option<Failure<E>>,
}

/// A job's failure and the index it failed at.
#[derive(Debug)]
struct Failure<E> {
    index: usize,
    error: E,
}

/// Run `job` over `0..len` with one slot bound to each in-flight index, at most `slots.len()` of
/// them at a time, and splice the results back into index order.
///
/// One scoped task per slot, each taking the next index the moment it frees up. That rolling
/// window is the point: batching the indices instead would make every window wait on its slowest
/// member, and these jobs are RAW decodes and warps whose costs differ by a lot.
///
/// A failure stops workers from running indices past it. Ones already running still finish, so
/// the bound on wasted work is a slot's worth, not zero.
///
/// Of several failures the one at the lowest index is returned, the failure a sequential map
/// would return: a worker skips only an index above the lowest failure seen, so every index below
/// the lowest runs to its end.
pub(crate) fn try_par_map_bounded<S, R, E>(
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

    let next = AtomicUsize::new(0);
    let lowest_failure = AtomicUsize::new(usize::MAX);
    let mut outcomes: Vec<SlotOutcome<R, E>> = slots
        .iter()
        .map(|_| SlotOutcome {
            values: Vec::new(),
            failure: None,
        })
        .collect();

    // `scope` + one `spawn` per slot rather than `slots.par_iter_mut()`: rayon splits a parallel
    // iterator only while threads are idle, so on a busy pool it could hand every slot to a
    // single task, whose worker loop would then drain the whole index range by itself.
    rayon::scope(|scope| {
        for (slot, outcome) in slots.iter_mut().zip(outcomes.iter_mut()) {
            let (next, lowest_failure, job) = (&next, &lowest_failure, &job);
            scope.spawn(move |_| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= len || index > lowest_failure.load(Ordering::Acquire) {
                        break;
                    }
                    match job(slot, index) {
                        Ok(value) => outcome.values.push((index, value)),
                        Err(error) => {
                            lowest_failure.fetch_min(index, Ordering::AcqRel);
                            outcome.failure = Some(Failure { index, error });
                            return;
                        }
                    }
                }
            });
        }
    });

    let mut ordered: Vec<Option<R>> = (0..len).map(|_| None).collect();
    let mut first_failure: Option<Failure<E>> = None;
    for outcome in outcomes {
        for (index, value) in outcome.values {
            ordered[index] = Some(value);
        }
        if let Some(failure) = outcome.failure
            && first_failure
                .as_ref()
                .is_none_or(|first| failure.index < first.index)
        {
            first_failure = Some(failure);
        }
    }
    if let Some(failure) = first_failure {
        return Err(failure.error);
    }
    Ok(ordered
        .into_iter()
        .map(|value| value.expect("each index below len is claimed by exactly one worker"))
        .collect())
}

/// Maps a fallible operation over `items`, at most `max_concurrent` at a time, passing each
/// item's index alongside it.
///
/// The index is supplied because callers almost always need it — to name a spill file, to report
/// which frame failed — and would otherwise each build a `Vec<(usize, &T)>` to carry it in.
/// See [`try_par_map_bounded`] for the scheduling and the early-exit bound.
pub(crate) fn try_par_map_limited<T, R, E, F>(
    items: &[T],
    max_concurrent: usize,
    operation: F,
) -> Result<Vec<R>, E>
where
    T: Sync,
    R: Send,
    E: Send,
    F: Fn(usize, &T) -> Result<R, E> + Sync,
{
    assert!(max_concurrent > 0, "max_concurrent must be positive");
    let mut slots = vec![(); max_concurrent];
    try_par_map_bounded(items.len(), &mut slots, |(), index| {
        operation(index, &items[index])
    })
}

/// Consuming counterpart to [`try_par_map_limited`], with a slot bound to each in-flight worker.
///
/// Taking items by value is what lets a caller drop each input as soon as its output exists — the
/// property that keeps the register/warp stage from holding the whole input and output sets
/// simultaneously. The cells outlive the run but each holds `None` once claimed, so what stays
/// resident is one lock and one `Option` discriminant per item, not the payload.
///
/// The slot is what a worker carries *between* the items it happens to take: scratch too big to
/// rebuild per item, and too plentiful to give one to every item. The warp stage keeps its output
/// planes there — a frame's worth of buffers whose pages are already faulted in, worth about a
/// fifth of a large frame's warp.
pub(crate) fn try_par_map_bounded_owned<T, S, R, E, F>(
    items: Vec<T>,
    slots: &mut [S],
    operation: F,
) -> Result<Vec<R>, E>
where
    T: Send,
    S: Send,
    R: Send,
    E: Send,
    F: Fn(&mut S, usize, T) -> Result<R, E> + Sync,
{
    let cells: Vec<Mutex<Option<T>>> = items
        .into_iter()
        .map(|item| Mutex::new(Some(item)))
        .collect();
    try_par_map_bounded(cells.len(), slots, |slot, index| {
        let item = cells[index]
            .lock()
            .expect("no holder of this lock panicked")
            .take()
            .expect("each index is claimed by exactly one worker");
        operation(slot, index, item)
    })
}

#[cfg(test)]
mod tests;
