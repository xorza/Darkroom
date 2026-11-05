//! Locks that panic on poison: a poisoned lock means a panic left its data half-changed, so the
//! thread that takes it panics too (`AGENTS.md`, "A poisoned lock is a logic error").
#![expect(
    clippy::disallowed_methods,
    reason = "the one place a std lock is taken and its poison checked"
)]

use std::sync::{Condvar, Mutex, MutexGuard, TryLockError};

const POISONED: &str = "a holder of this lock panicked: the panic above left its data half-changed";

/// [`Mutex`]'s calls, each panicking on a poisoned lock.
pub trait MutexExt<T: ?Sized> {
    /// [`Mutex::lock`].
    ///
    /// # Panics
    ///
    /// When a panic poisoned the lock.
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;

    /// [`Mutex::try_lock`]; `None` while another holds it.
    ///
    /// # Panics
    ///
    /// When a panic poisoned the lock.
    fn try_lock_unpoisoned(&self) -> Option<MutexGuard<'_, T>>;

    /// [`Mutex::get_mut`].
    ///
    /// # Panics
    ///
    /// When a panic poisoned the lock.
    fn get_mut_unpoisoned(&mut self) -> &mut T;
}

impl<T: ?Sized> MutexExt<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|_| panic!("{POISONED}"))
    }

    fn try_lock_unpoisoned(&self) -> Option<MutexGuard<'_, T>> {
        match self.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => panic!("{POISONED}"),
        }
    }

    fn get_mut_unpoisoned(&mut self) -> &mut T {
        self.get_mut().unwrap_or_else(|_| panic!("{POISONED}"))
    }
}

/// [`Condvar`]'s waits, each panicking on the poisoned lock it takes back.
pub trait CondvarExt {
    /// [`Condvar::wait`].
    ///
    /// # Panics
    ///
    /// When a panic poisoned the lock.
    fn wait_unpoisoned<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T>;

    /// [`Condvar::wait_while`].
    ///
    /// # Panics
    ///
    /// When a panic poisoned the lock.
    fn wait_while_unpoisoned<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        condition: impl FnMut(&mut T) -> bool,
    ) -> MutexGuard<'a, T>;
}

impl CondvarExt for Condvar {
    fn wait_unpoisoned<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        self.wait(guard).unwrap_or_else(|_| panic!("{POISONED}"))
    }

    fn wait_while_unpoisoned<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        condition: impl FnMut(&mut T) -> bool,
    ) -> MutexGuard<'a, T> {
        self.wait_while(guard, condition)
            .unwrap_or_else(|_| panic!("{POISONED}"))
    }
}

#[cfg(test)]
mod tests;
