//! Concurrency helpers for Rayon work and reusable per-job resources.

use std::ops::Deref;
use std::ops::DerefMut;

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
    pub(crate) fn acquire(&self) -> JobScratchLease<'_, T> {
        let value = self.values.lock().pop().unwrap_or_default();
        JobScratchLease {
            value: Some(value),
            pool: &self.values,
        }
    }
}

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

/// Maps a fallible operation over consecutive parallel batches, passing each item's index
/// alongside it.
///
/// At most `max_concurrent` operations run at once. Results preserve input order, and batches
/// after the first error are not started. The index is supplied because callers almost always
/// need it — to name a spill file, to report which frame failed — and would otherwise each
/// build a `Vec<(usize, &T)>` to carry it in.
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

    let mut results = Vec::with_capacity(items.len());
    for (batch, chunk) in items.chunks(max_concurrent).enumerate() {
        let base = batch * max_concurrent;
        results.extend(
            chunk
                .par_iter()
                .enumerate()
                .map(|(offset, item)| operation(base + offset, item))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(results)
}

/// Consuming counterpart to [`try_par_map_limited`]: maps a fallible operation over `items`,
/// handing each one to `operation` by value along with its input index.
///
/// At most `max_concurrent` operations run at once. Results preserve input order, and batches
/// after the first error are not started. Taking items by value is what lets a caller drop each
/// input as soon as its output exists — the property that keeps the register/warp stage from
/// holding the whole input and output sets simultaneously.
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
    assert!(max_concurrent > 0, "max_concurrent must be positive");

    let mut results = Vec::with_capacity(items.len());
    let mut pending = items.into_iter().enumerate();
    loop {
        let batch: Vec<(usize, T)> = pending.by_ref().take(max_concurrent).collect();
        if batch.is_empty() {
            return Ok(results);
        }
        results.extend(
            batch
                .into_par_iter()
                .map(|(index, item)| operation(index, item))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
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
