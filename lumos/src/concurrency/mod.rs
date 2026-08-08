//! Concurrency helpers for Rayon work and reusable per-job resources.

use std::ops::Deref;
use std::ops::DerefMut;
use std::ops::Range;

use parking_lot::Mutex;
use rayon::prelude::*;

/// Wrapper to send raw pointers across thread boundaries in Rayon closures.
///
/// SAFETY: Caller must ensure disjoint access from each thread.
///
/// Access the inner value via `.get()` — never `.0` — so that Edition 2024
/// closures capture `&UnsafeSendPtr` (which is Sync) rather than the inner
/// pointer field.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnsafeSendPtr<T: Copy>(T);
unsafe impl<T: Copy> Send for UnsafeSendPtr<T> {}
unsafe impl<T: Copy> Sync for UnsafeSendPtr<T> {}

impl<T: Copy> UnsafeSendPtr<T> {
    pub(crate) fn new(ptr: T) -> Self {
        Self(ptr)
    }

    pub(crate) fn get(&self) -> T {
        self.0
    }
}

/// The `for_each_init` init for scratch that has to outlive the parallel call.
///
/// Rayon runs an init closure once per worker and drops what it returns when the call ends,
/// which is the right shape when the call *is* the operation — a demosaic pass allocates its row
/// buffers straight into the init (`io/raw/demosaic/xtrans/markesteijn_steps.rs`) because nothing
/// in the RAW path outlives one frame. Reach for a pool only when the same loop runs many times
/// over: once per chunk per channel in the combine, once per tile row in the background mesh.
/// Then the init becomes `|| pool.acquire()` and the lease hands its value back on drop, so the
/// next call finds it warm. Both are the same mechanism; the pool is just a smarter init.
///
/// Values come back with **unspecified contents** — a fresh one is `Default`, a reused one keeps
/// whatever the last holder left in it. Size or clear on acquire.
#[derive(Debug)]
pub(crate) struct JobScratchPool<T> {
    values: Mutex<Vec<T>>,
}

impl<T> Default for JobScratchPool<T> {
    fn default() -> Self {
        Self {
            values: Mutex::new(Vec::new()),
        }
    }
}

impl<T: Default> JobScratchPool<T> {
    /// Take a value from the pool, or build a fresh one when it is empty.
    pub(crate) fn acquire(&self) -> JobScratchLease<'_, T> {
        let value = self.values.lock().pop().unwrap_or_default();
        JobScratchLease {
            value: Some(value),
            pool: &self.values,
        }
    }
}

/// A value on loan from a [`JobScratchPool`], returned to it when dropped.
#[derive(Debug)]
pub(crate) struct JobScratchLease<'a, T> {
    value: Option<T>,
    pool: &'a Mutex<Vec<T>>,
}

impl<T> Deref for JobScratchLease<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref().unwrap()
    }
}

impl<T> DerefMut for JobScratchLease<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut().unwrap()
    }
}

impl<T> Drop for JobScratchLease<'_, T> {
    fn drop(&mut self) {
        self.pool.lock().push(self.value.take().unwrap());
    }
}

/// Splice the results of `batch` over consecutive windows of at most `max_concurrent` of `len`
/// items, in input order.
///
/// The window boundary is a barrier: one batch finishes before the next starts, and the first to
/// fail leaves the rest unstarted. Callers supply only the parallel iterator over a window — the
/// ordering, the window width, and the early exit live here rather than being spelled out again
/// in each entry point.
pub(crate) fn try_collect_batches<R, E>(
    len: usize,
    max_concurrent: usize,
    mut batch: impl FnMut(Range<usize>) -> Result<Vec<R>, E>,
) -> Result<Vec<R>, E> {
    assert!(max_concurrent > 0, "max_concurrent must be positive");

    let mut results = Vec::with_capacity(len);
    let mut start = 0;
    while start < len {
        let end = (start + max_concurrent).min(len);
        results.extend(batch(start..end)?);
        start = end;
    }
    Ok(results)
}

/// Maps a fallible operation over `items` in [`try_collect_batches`] windows, passing each item's
/// index alongside it.
///
/// The index is supplied because callers almost always need it — to name a spill file, to report
/// which frame failed — and would otherwise each build a `Vec<(usize, &T)>` to carry it in.
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
    try_collect_batches(items.len(), max_concurrent, |batch| {
        items[batch.start..batch.end]
            .par_iter()
            .enumerate()
            .map(|(offset, item)| operation(batch.start + offset, item))
            .collect()
    })
}

/// Consuming counterpart to [`try_par_map_limited`]: hands each item to `operation` by value.
///
/// Taking items by value is what lets a caller drop each input as soon as its output exists —
/// the property that keeps the register/warp stage from holding the whole input and output sets
/// simultaneously. Each window is drained off the input iterator just before it runs, so items
/// beyond the current window stay untouched until their turn.
pub(crate) fn try_par_map_limited_owned<T, R, E, F>(
    items: Vec<T>,
    max_concurrent: usize,
    operation: F,
) -> Result<Vec<R>, E>
where
    T: Send,
    R: Send,
    E: Send,
    F: Fn(usize, T) -> Result<R, E> + Sync,
{
    let len = items.len();
    let mut pending = items.into_iter();
    try_collect_batches(len, max_concurrent, |batch| {
        pending
            .by_ref()
            .take(batch.len())
            .collect::<Vec<T>>()
            .into_par_iter()
            .enumerate()
            .map(|(offset, item)| operation(batch.start + offset, item))
            .collect()
    })
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::concurrency::JobScratchPool;

    pub(crate) fn job_count<T>(pool: &JobScratchPool<T>) -> usize {
        pool.values.lock().len()
    }

    pub(crate) fn all_by<T>(pool: &JobScratchPool<T>, predicate: impl Fn(&T) -> bool) -> bool {
        pool.values.lock().iter().all(predicate)
    }
}

#[cfg(test)]
mod tests;
