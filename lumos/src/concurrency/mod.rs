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

/// Maps a fallible operation over consecutive parallel batches.
///
/// At most `max_concurrent` operations run at once. Results preserve input
/// order, and batches after the first error are not started.
pub(crate) fn try_par_map_limited<T, R, E, F>(
    items: &[T],
    max_concurrent: usize,
    operation: F,
) -> Result<Vec<R>, E>
where
    T: Sync,
    R: Send,
    E: Send,
    F: Fn(&T) -> Result<R, E> + Sync,
{
    assert!(max_concurrent > 0, "max_concurrent must be positive");

    let mut results = Vec::with_capacity(items.len());
    for chunk in items.chunks(max_concurrent) {
        results.extend(
            chunk
                .par_iter()
                .map(&operation)
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(results)
}

#[cfg(test)]
pub(crate) mod test_support {
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
