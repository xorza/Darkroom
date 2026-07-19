//! Concurrency helpers for Rayon work and reusable per-job resources.

use parking_lot::Mutex;
use std::ops::Deref;
use std::ops::DerefMut;

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

#[cfg(test)]
pub(crate) mod test_support {
    use crate::concurrency::JobScratchPool;

    pub(crate) fn job_count<T>(pool: &JobScratchPool<T>) -> usize {
        pool.values.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use crate::concurrency::JobScratchPool;

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
}
