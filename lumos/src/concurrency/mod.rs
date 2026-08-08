//! Concurrency helpers for Rayon work and reusable per-job resources.

use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::Mutex;

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

/// Run `job` over `0..len` with one slot bound to each in-flight index, at most `slots.len()` of
/// them at a time, and splice the results back into index order.
///
/// One scoped task per slot, each taking the next index the moment it frees up. That rolling
/// window is the point: batching the indices instead would make every window wait on its slowest
/// member, and these jobs are RAW decodes and warps whose costs differ by a lot.
///
/// The first failure stops workers from *taking* further indices. Ones already running still
/// finish, and a worker that read the index counter just before the failure landed may run one
/// more — so the bound on wasted work is a slot's worth, not zero.
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
    assert!(!slots.is_empty(), "max_concurrent must be positive");

    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let mut outcomes: Vec<Result<Vec<(usize, R)>, E>> =
        slots.iter().map(|_| Ok(Vec::new())).collect();

    // `scope` + one `spawn` per slot rather than `slots.par_iter_mut()`: rayon splits a parallel
    // iterator only while threads are idle, so on a busy pool it could hand every slot to a
    // single task, whose worker loop would then drain the whole index range by itself.
    rayon::scope(|scope| {
        for (slot, outcome) in slots.iter_mut().zip(outcomes.iter_mut()) {
            let (next, failed, job) = (&next, &failed, &job);
            scope.spawn(move |_| {
                let mut mine = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= len || failed.load(Ordering::Relaxed) {
                        break;
                    }
                    match job(slot, index) {
                        Ok(value) => mine.push((index, value)),
                        Err(error) => {
                            failed.store(true, Ordering::Relaxed);
                            *outcome = Err(error);
                            return;
                        }
                    }
                }
                *outcome = Ok(mine);
            });
        }
    });

    let mut ordered: Vec<Option<R>> = (0..len).map(|_| None).collect();
    for outcome in outcomes {
        for (index, value) in outcome? {
            ordered[index] = Some(value);
        }
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
    let mut slots = vec![(); max_concurrent];
    try_par_map_bounded(items.len(), &mut slots, |(), index| {
        operation(index, &items[index])
    })
}

/// Consuming counterpart to [`try_par_map_limited`]: hands each item to `operation` by value.
///
/// Taking items by value is what lets a caller drop each input as soon as its output exists —
/// the property that keeps the register/warp stage from holding the whole input and output sets
/// simultaneously. The cells outlive the run but each holds `None` once claimed, so what stays
/// resident is one lock and one `Option` discriminant per item, not the payload.
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
    let cells: Vec<Mutex<Option<T>>> = items
        .into_iter()
        .map(|item| Mutex::new(Some(item)))
        .collect();
    try_par_map_limited(&cells, max_concurrent, |index, cell| {
        let item = cell
            .lock()
            .take()
            .expect("each index is claimed by exactly one worker");
        operation(index, item)
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
