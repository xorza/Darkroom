use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use rayon::prelude::*;

use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;
use crate::progress::stage_counter::StageCounter;

/// Workers finishing in any order still hand the callback `1..=total`, in order.
#[test]
fn parallel_completions_report_in_order() {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let callback = ProgressCallback::new({
        let reports = Arc::clone(&reports);
        move |progress| {
            reports.lock().unwrap().push((progress.cur, progress.total));
        }
    });
    let counter = StageCounter::new(&callback, StackingStage::Loading, 64);
    (0..64).into_par_iter().for_each(|_| {
        counter.complete_one();
    });
    let expected: Vec<_> = (1..=64).map(|cur| (cur, 64)).collect();
    assert_eq!(*reports.lock().unwrap(), expected);
}

/// A callback that runs rayon work of its own still sees every count in order: a worker waiting
/// for the parts of its loop that others took steals units of the stage meanwhile, which
/// complete on the same thread and must not wait for the report they are inside. A watchdog
/// fails the test where a deadlock would hang it.
#[test]
fn a_callback_that_uses_rayon_completes() {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let callback = ProgressCallback::new({
            let reports = Arc::clone(&reports);
            move |progress| {
                (0..8).into_par_iter().for_each(|_| {
                    thread::sleep(Duration::from_micros(20));
                });
                reports.lock().unwrap().push(progress.cur);
            }
        });
        let counter = StageCounter::new(&callback, StackingStage::Loading, 256);
        (0..256).into_par_iter().for_each(|_| {
            counter.complete_one();
        });
        sender.send(reports.lock().unwrap().clone()).unwrap();
    });
    let reports = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the stage deadlocked in its progress callback");
    assert_eq!(reports, (1..=256).collect::<Vec<_>>());
}
