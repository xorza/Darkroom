//! Parallel work, started from a token a loop's body cannot hold.
//!
//! A worker that blocks in a rayon call runs other queued jobs on its own stack, so a parallel
//! loop started inside another loop's body puts that body's state under unrelated work: per-worker
//! scratch stops being exclusive and every per-worker memory figure is wrong. Rayon has no way to
//! isolate the inner work (oneTBB's `this_task_arena::isolate` is the reference answer), so the
//! rule is that a loop body never starts parallel work, and the types enforce it for every loop
//! started from a [`Par`]: each entry point is one of its methods, and a body cannot hold one.

pub(super) mod per_worker;
pub(super) mod worker_pool;

use std::marker::PhantomData;
use std::ops::RangeBounds;
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::ThreadPool;
use rayon::iter::{
    IntoParallelIterator, IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelDrainRange,
    ParallelExtend,
};
use rayon::slice::{
    Chunks, ChunksExact, ChunksExactMut, ChunksMut, ParallelSlice, ParallelSliceMut,
};

/// The right to start parallel work, held by code that is not a parallel loop's body.
///
/// A [`WorkerPool`](crate::WorkerPool) hands out the first one; a function that runs parallel work
/// takes `par: &Par<'_>` from its caller. `Par` is neither `Send` nor `Sync`, and rayon requires
/// every body to be both, so a body can neither capture the token nor have it moved in: a
/// parallel loop inside a body does not compile.
#[derive(Debug)]
pub struct Par<'p> {
    pool: &'p ThreadPool,
    not_send: PhantomData<*const ()>,
}

impl<'p> Par<'p> {
    pub(crate) const fn new(pool: &'p ThreadPool) -> Self {
        Self {
            pool,
            not_send: PhantomData,
        }
    }

    /// The workers this run spreads over, the count a per-worker memory figure multiplies.
    pub fn threads(&self) -> usize {
        self.pool.current_num_threads()
    }

    /// `items.par_iter()`.
    #[expect(
        clippy::iter_not_returning_iterator,
        reason = "rayon's parallel iterator, under rayon's name less its `par_`"
    )]
    pub fn iter<'d, C: IntoParallelRefIterator<'d> + ?Sized>(&self, items: &'d C) -> C::Iter {
        items.par_iter()
    }

    /// `items.par_iter_mut()`.
    #[expect(
        clippy::iter_not_returning_iterator,
        reason = "rayon's parallel iterator, under rayon's name less its `par_`"
    )]
    pub fn iter_mut<'d, C: IntoParallelRefMutIterator<'d> + ?Sized>(
        &self,
        items: &'d mut C,
    ) -> C::Iter {
        items.par_iter_mut()
    }

    /// `items.into_par_iter()`.
    pub fn into_iter<I: IntoParallelIterator>(&self, items: I) -> I::Iter {
        items.into_par_iter()
    }

    /// `items.par_chunks(size)`.
    pub fn chunks<'d, T: Sync>(&self, items: &'d [T], size: usize) -> Chunks<'d, T> {
        items.par_chunks(size)
    }

    /// `items.par_chunks_exact(size)`.
    pub fn chunks_exact<'d, T: Sync>(&self, items: &'d [T], size: usize) -> ChunksExact<'d, T> {
        items.par_chunks_exact(size)
    }

    /// `items.par_chunks_mut(size)`.
    pub fn chunks_mut<'d, T: Send>(&self, items: &'d mut [T], size: usize) -> ChunksMut<'d, T> {
        items.par_chunks_mut(size)
    }

    /// `items.par_chunks_exact_mut(size)`.
    pub fn chunks_exact_mut<'d, T: Send>(
        &self,
        items: &'d mut [T],
        size: usize,
    ) -> ChunksExactMut<'d, T> {
        items.par_chunks_exact_mut(size)
    }

    /// `items.par_drain(range)`.
    pub fn drain<C: ParallelDrainRange<usize>>(
        &self,
        items: C,
        range: impl RangeBounds<usize>,
    ) -> C::Iter {
        items.par_drain(range)
    }

    /// `target.par_extend(items)`.
    pub fn extend<T: Send, C: ParallelExtend<T> + ?Sized>(
        &self,
        target: &mut C,
        items: impl IntoParallelIterator<Item = T>,
    ) {
        target.par_extend(items);
    }

    /// Runs `job` over `0..len` with one slot bound to each in-flight index, at most
    /// `slots.len()` at a time, and splices the results back into index order.
    ///
    /// The one place parallel work nests: each job gets a [`Par`] of its own, so a job is a
    /// whole stage of work (a frame's decode, a frame's warp) whose loops run in parallel too.
    /// A job a worker takes while it waits inside another sits on that worker's stack, but each
    /// owns its slot, so the memory in flight stays within the slot count. Its cost is that a job
    /// finishes only when the jobs above it on the stack do (rayon #1054).
    ///
    /// One task per slot, each taking the next index the moment it frees up: batching the indices
    /// instead would make every batch wait on its slowest member. A failure stops the slots from
    /// taking indices past it; those already running finish. Of several failures the one at the
    /// lowest index is returned, the failure a sequential map would return.
    ///
    /// # Panics
    /// With no slot.
    pub fn slots<S, R, E>(
        &self,
        len: usize,
        slots: &mut [S],
        job: impl Fn(&mut S, usize, &Par<'_>) -> Result<R, E> + Sync,
    ) -> Result<Vec<R>, E>
    where
        S: Send,
        R: Send,
        E: Send,
    {
        assert!(!slots.is_empty(), "a bounded map needs at least one slot");
        let next = AtomicUsize::new(0);
        let lowest_failure = AtomicUsize::new(usize::MAX);
        let mut outcomes: Vec<SlotOutcome<R, E>> =
            slots.iter().map(|_| SlotOutcome::default()).collect();
        let pool = self.pool;
        // `scope` + one `spawn` per slot rather than a parallel iterator over the slots: rayon
        // splits an iterator only while threads are idle, so on a busy pool one task could take
        // every slot and drain the whole index range alone.
        rayon::scope(|scope| {
            for (slot, outcome) in slots.iter_mut().zip(&mut outcomes) {
                let (next, lowest_failure, job) = (&next, &lowest_failure, &job);
                scope.spawn(move |_| {
                    let par = Par::new(pool);
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        if idx >= len || idx > lowest_failure.load(Ordering::Acquire) {
                            break;
                        }
                        match job(slot, idx, &par) {
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
}

/// What one slot of [`Par::slots`] finished with: the values of the indices it took, and the
/// failure that stopped it, if one did.
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
