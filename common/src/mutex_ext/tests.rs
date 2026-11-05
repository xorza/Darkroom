use crate::mutex_ext::{CondvarExt, MutexExt, POISONED};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Condvar, Mutex};
use std::thread;

/// `m` poisoned by a panic that unwinds through its guard.
fn poison<T>(m: &Mutex<T>) {
    let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
        let _held = m.lock_unpoisoned();
        panic!("poison the lock");
    }));
    assert!(unwound.is_err());
    assert!(m.is_poisoned());
}

/// The message `call` panics with, which must be one.
fn panic_message(call: impl FnOnce()) -> String {
    let payload = panic::catch_unwind(AssertUnwindSafe(call)).expect_err("the call panics");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .expect("a text message")
}

/// A healthy lock gives its value to each call: 7, then 8 after a write, and 9 through `get_mut`;
/// a free `try_lock` gives the guard and a held one `None`. Once a panic poisons it, each call
/// panics with [`POISONED`], `try_lock` included.
#[test]
fn a_poisoned_mutex_panics_on_every_call() {
    let mut m = Mutex::new(7);
    assert_eq!(*m.lock_unpoisoned(), 7);
    *m.lock_unpoisoned() = 8;
    assert_eq!(m.try_lock_unpoisoned().map(|guard| *guard), Some(8));
    {
        let _held = m.lock_unpoisoned();
        assert!(m.try_lock_unpoisoned().is_none(), "held by this thread");
    }
    *m.get_mut_unpoisoned() = 9;
    assert_eq!(*m.lock_unpoisoned(), 9);

    poison(&m);
    assert_eq!(panic_message(|| drop(m.lock_unpoisoned())), POISONED);
    assert_eq!(panic_message(|| drop(m.try_lock_unpoisoned())), POISONED);
    assert_eq!(
        panic_message(|| {
            let _ = m.get_mut_unpoisoned();
        }),
        POISONED
    );
}

/// A wait on a healthy lock sleeps until a second thread sets the flag and wakes it, through
/// `wait_while` and through a `wait` loop alike; a wait that takes back a lock a panic poisoned
/// meanwhile panics with [`POISONED`].
#[test]
fn a_wait_panics_when_it_takes_back_a_poisoned_lock() {
    for use_wait_while in [true, false] {
        let flag = Mutex::new(0);
        let wake = Condvar::new();
        thread::scope(|scope| {
            let guard = flag.lock_unpoisoned();
            scope.spawn(|| {
                *flag.lock_unpoisoned() = 1;
                wake.notify_one();
            });
            let guard = if use_wait_while {
                wake.wait_while_unpoisoned(guard, |flag| *flag == 0)
            } else {
                let mut guard = guard;
                while *guard == 0 {
                    guard = wake.wait_unpoisoned(guard);
                }
                guard
            };
            assert_eq!(*guard, 1);
        });

        let message = panic_message(|| {
            thread::scope(|scope| {
                let guard = flag.lock_unpoisoned();
                scope.spawn(|| {
                    poison(&flag);
                    wake.notify_one();
                });
                if use_wait_while {
                    drop(wake.wait_while_unpoisoned(guard, |_| true));
                } else {
                    let mut guard = guard;
                    while *guard == 1 {
                        guard = wake.wait_unpoisoned(guard);
                    }
                }
            });
        });
        assert_eq!(message, POISONED);
    }
}
