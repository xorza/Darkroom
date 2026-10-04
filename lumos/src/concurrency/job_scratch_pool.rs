//! [`JobScratchPool`]: scratch that outlives one parallel call, leased per job.

use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::Mutex;

/// The `for_each_init` init for scratch that has to outlive the parallel call.
///
/// Rayon runs an init closure once per worker and drops what it returns when the call ends,
/// which is the right shape when the call *is* the operation — the X-Trans demosaic allocates its
/// tile buffers straight into the init (`io/raw/demosaic/xtrans/markesteijn/mod.rs`) because
/// nothing in the RAW path outlives one frame. Reach for a pool only when the same loop runs many times
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
        let value = self
            .values
            .lock()
            .expect("no holder of this lock panicked")
            .pop()
            .unwrap_or_default();
        JobScratchLease {
            value: ManuallyDrop::new(value),
            pool: &self.values,
        }
    }
}

/// A value on loan from a [`JobScratchPool`], returned to it when dropped.
#[derive(Debug)]
pub(crate) struct JobScratchLease<'a, T> {
    /// Held for the lease's whole life and moved back into the pool by `drop`, which is why it is
    /// not dropped in place.
    value: ManuallyDrop<T>,
    pool: &'a Mutex<Vec<T>>,
}

impl<T> Deref for JobScratchLease<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> DerefMut for JobScratchLease<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl<T> Drop for JobScratchLease<'_, T> {
    fn drop(&mut self) {
        // SAFETY: `value` is taken exactly once, here, and the lease is never read after its drop.
        let value = unsafe { ManuallyDrop::take(&mut self.value) };
        self.pool
            .lock()
            .expect("no holder of this lock panicked")
            .push(value);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::concurrency::job_scratch_pool::JobScratchPool;

    pub(crate) fn job_count<T>(pool: &JobScratchPool<T>) -> usize {
        pool.values
            .lock()
            .expect("no holder of this lock panicked")
            .len()
    }

    pub(crate) fn all_by<T>(pool: &JobScratchPool<T>, predicate: impl Fn(&T) -> bool) -> bool {
        pool.values
            .lock()
            .expect("no holder of this lock panicked")
            .iter()
            .all(predicate)
    }
}
