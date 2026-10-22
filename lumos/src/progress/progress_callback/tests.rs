use std::sync::{Arc, Mutex};

use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::{StackingProgress, StackingStage};

#[test]
fn callback_reports_exact_progress_and_default_is_silent() {
    ProgressCallback::default().report(1, 2, StackingStage::Loading);

    let reports = Arc::new(Mutex::new(Vec::new()));
    let callback = ProgressCallback::new({
        let reports = Arc::clone(&reports);
        move |progress| reports.lock().unwrap().push(progress)
    });
    callback.report(3, 5, StackingStage::Combining);

    let reports = reports.lock().unwrap();
    let [StackingProgress { cur, total, stage }] = reports.as_slice() else {
        panic!("expected one progress report");
    };
    assert_eq!((*cur, *total, *stage), (3, 5, StackingStage::Combining));
}
