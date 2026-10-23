//! Concurrency helpers for Rayon work.

pub(crate) mod unsafe_send_ptr;

use std::mem::MaybeUninit;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

/// Samples per rayon work item. Parallelizing per sample drowns a cheap per-pixel op in rayon's
/// recursive split/join overhead (it dominated SCNR); a coarse block amortizes that while staying
/// load-balanced and letting the inner loop auto-vectorize.
pub(crate) const SAMPLES_PER_BLOCK: usize = 8192;

/// How many samples a cancellable walk covers between cancel polls.
///
/// The frame checks and the normalization gather chunk their iteration to this rather than test
/// the token per sample. The figure is not tuned to either: it is a granularity, small enough that
/// a cancelled run stops promptly and large enough that the poll is lost in the work.
pub(crate) const CANCEL_POLL_CHUNK: usize = 16_384;

/// The `f32` samples of a 2 MiB transparent huge page.
const HUGE_PAGE_SAMPLES: usize = 2 * 1024 * 1024 / size_of::<f32>();

/// `len` zeros, written by the workers a 2 MiB block each, for a plane workers then fill in
/// contiguous pieces smaller than a page. A fresh plane's pages are zeroed by the kernel at the
/// first write to each; when such pieces make those first writes, several workers fault on one
/// huge page and wait on each other, which took two thirds of a 24 MP FITS decode. Where the first
/// writes already spread over the pages, as a demosaic's tiles do, this pass only adds work.
pub(crate) fn zeroed_in_parallel(len: usize) -> Vec<f32> {
    let mut plane = Vec::with_capacity(len);
    plane.spare_capacity_mut()[..len]
        .par_chunks_mut(HUGE_PAGE_SAMPLES)
        .for_each(|block| block.fill(MaybeUninit::new(0.0)));
    unsafe { plane.set_len(len) };
    plane
}

/// What one slot of a bounded map finished with: the values of the indices it took, and the
/// failure that stopped it, if one did.
#[derive(Debug)]
struct SlotOutcome<R, E> {
    values: Vec<Indexed<R>>,
    failure: Option<Failure<E>>,
}

/// A job's value and the index it ran for.
#[derive(Debug)]
struct Indexed<R> {
    idx: usize,
    value: R,
}

/// A job's failure and the index it failed at.
#[derive(Debug)]
struct Failure<E> {
    idx: usize,
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
    debug_assert!(!slots.is_empty(), "a bounded map needs at least one slot");

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
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    if idx >= len || idx > lowest_failure.load(Ordering::Acquire) {
                        break;
                    }
                    match job(slot, idx) {
                        Ok(value) => outcome.values.push(Indexed { idx, value }),
                        Err(error) => {
                            lowest_failure.fetch_min(idx, Ordering::AcqRel);
                            outcome.failure = Some(Failure { idx, error });
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
        return Err(failure.error);
    }
    Ok(ordered
        .into_iter()
        .map(|value| value.expect("each index below len is claimed by exactly one worker"))
        .collect())
}

/// [`try_par_map_bounded`] for jobs whose workers carry nothing from one index to the next: at
/// most `width` of them at a time.
pub(crate) fn try_par_map_at_most<R, E>(
    len: usize,
    width: usize,
    job: impl Fn(usize) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E>
where
    R: Send,
    E: Send,
{
    try_par_map_bounded(len, &mut vec![(); width], |(), idx| job(idx))
}

/// Consuming counterpart to [`try_par_map_bounded`]: each item moved into its job.
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
    try_par_map_bounded(cells.len(), slots, |slot, idx| {
        let item = cells[idx]
            .lock()
            .expect("no holder of this lock panicked")
            .take()
            .expect("each index is claimed by exactly one worker");
        operation(slot, idx, item)
    })
}

#[cfg(test)]
mod tests;
