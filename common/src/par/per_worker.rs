//! [`PerWorker`]: scratch a parallel loop's bodies take one worker's share of.

use std::sync::{Mutex, MutexGuard, TryLockError};

use crate::par::Par;

/// One `S` for each worker of a pool, kept by its owner from one run to the next, so a loop's
/// working set is exactly `workers × S` and a run allocates none of it. Rayon's own `map_init`
/// and its kin make a value for each split instead, a count no thread count bounds.
///
/// A body takes its worker's slot with [`get`](Self::get). A slot has one user at a time because
/// a body never starts parallel work, which is all that could put a second body on its worker's
/// stack; the borrow is checked anyway, so a guard held across a parallel call panics rather than
/// aliases.
#[derive(Debug)]
pub struct PerWorker<S> {
    slots: Box<[Slot<S>]>,
}

/// One worker's slot, alone on its cache lines: every body writes its slot's lock, and a lock
/// beside another worker's would make the two cores take the line from each other on each body.
/// 128 bytes covers the pair of lines x86's adjacent-line prefetch fetches together and Apple's
/// 128-byte lines.
#[derive(Debug)]
#[repr(align(128))]
struct Slot<S>(Mutex<S>);

impl<S> PerWorker<S> {
    /// A slot for each worker `par` runs on, each made by `init`.
    pub fn new(par: &Par<'_>, mut init: impl FnMut() -> S) -> Self {
        Self {
            slots: (0..par.threads())
                .map(|_| Slot(Mutex::new(init())))
                .collect(),
        }
    }

    /// The workers this holds a slot for.
    pub fn workers(&self) -> usize {
        self.slots.len()
    }

    /// The calling worker's slot.
    ///
    /// # Panics
    /// Off a pool's worker, on a worker past [`workers`](Self::workers), as on a larger pool than
    /// the one it was made for, or while the slot is already taken.
    pub fn get(&self) -> MutexGuard<'_, S> {
        let idx = rayon::current_thread_index().expect("a worker's slot is taken on a worker");
        let slot = self.slots.get(idx).unwrap_or_else(|| {
            panic!(
                "worker {idx} has no slot of {}: made for a smaller pool",
                self.workers()
            )
        });
        match slot.0.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => {
                panic!(
                    "worker {idx}'s slot is taken: a guard held across parallel work, or slots \
                     two pools share"
                )
            }
            Err(TryLockError::Poisoned(_)) => {
                panic!("a body that held worker {idx}'s slot panicked")
            }
        }
    }

    /// Every slot, for the owner to read or reset between runs.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = &mut S> {
        self.slots
            .iter_mut()
            .map(|slot| slot.0.get_mut().expect("no body that held a slot panicked"))
    }
}
