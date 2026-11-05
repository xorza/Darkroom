//! [`ReportMailbox`]: the worker's reports the next frame has not taken, merged as they arrive.

use std::collections::VecDeque;
use std::mem;
use std::sync::Mutex;

use common::MutexExt;
use scenarium::{WorkerError, WorkerReport};

/// The reports since the last drain, each merged as it is posted: a report the
/// [fold](crate::gui::state::run_state::RunState) would overwrite before a frame shows it goes,
/// and the runs that ended in it are carried as a count. Only a frame drains it, and a hidden
/// window runs none, so a queue that kept each report would grow for as long as an event loop
/// runs behind it. This one holds, back to the last structural report, at most one `Activity`,
/// one `Completed`, one run error and the progress of the run after them; a structural report
/// answers a command or ends the event loop, so the commands bound their count. Refused: pausing
/// the event loops while the window is hidden, as a browser stops `requestAnimationFrame`, which
/// would stop a Watch Directory too; a `RunState` shared with the worker thread.
///
/// What a report logs is logged before it is posted ([`WorkerBridge`]), so a merge loses no log
/// line.
///
/// [`WorkerBridge`]: crate::core::worker_bridge::WorkerBridge
#[derive(Debug, Default)]
pub(crate) struct ReportMailbox {
    pending: Mutex<Pending>,
}

#[derive(Debug, Default)]
struct Pending {
    reports: VecDeque<WorkerReport>,
    /// The runs that ended in a `Completed` or a run error a later one superseded.
    superseded_runs: u64,
    /// Every report posted, superseded or not.
    #[cfg(test)]
    posts: usize,
}

impl ReportMailbox {
    /// Add `report`, after removing what it supersedes since the last structural report.
    pub(crate) fn post(&self, report: WorkerReport) {
        let mut guard = self.pending.lock_unpoisoned();
        let pending = &mut *guard;
        let since = pending
            .reports
            .iter()
            .rposition(is_structural)
            .map_or(0, |at| at + 1);
        let mut at = 0;
        pending.reports.retain(|held| {
            let superseded = at >= since && supersedes(&report, held);
            at += 1;
            if superseded && ends_a_run(held) {
                pending.superseded_runs += 1;
            }
            !superseded
        });
        pending.reports.push_back(report);
        #[cfg(test)]
        {
            pending.posts += 1;
        }
    }

    /// Swap the reports since the last drain into `reports`, which is empty, so the next posts
    /// fill its buffer; the runs that ended in a superseded report.
    pub(crate) fn drain(&self, reports: &mut VecDeque<WorkerReport>) -> u64 {
        debug_assert!(reports.is_empty(), "the last drain was folded whole");
        let mut pending = self.pending.lock_unpoisoned();
        mem::swap(reports, &mut pending.reports);
        mem::take(&mut pending.superseded_runs)
    }
}

/// A report no later one supersedes across: a program installed or cleared, or a worker error
/// that ends no run.
const fn is_structural(report: &WorkerReport) -> bool {
    match report {
        WorkerReport::Installed { .. } | WorkerReport::Cleared => true,
        WorkerReport::Error(error) => !matches!(error, WorkerError::Exec { .. }),
        WorkerReport::Activity(_) | WorkerReport::Progress { .. } | WorkerReport::Completed(_) => {
            false
        }
    }
}

/// A report the fold counts as a run that ended.
const fn ends_a_run(report: &WorkerReport) -> bool {
    matches!(
        report,
        WorkerReport::Completed(_) | WorkerReport::Error(WorkerError::Exec { .. })
    )
}

/// Whether folding `new` after `old` leaves what folding `new` alone leaves, the run count aside:
/// an `Activity` overwrites the activity; a `Completed` replaces every node's results, and with
/// them every `Progress`, the activity, and the summary and RAM of an earlier one; a run error clears
/// the nodes, so a `Progress` and an earlier run error with them. Refused: a `Completed` over a
/// run error, which clears the previews a `Completed` keeps, and the other way round, as a run
/// error keeps the RAM and activity a `Completed` sets.
const fn supersedes(new: &WorkerReport, old: &WorkerReport) -> bool {
    match new {
        WorkerReport::Activity(_) => matches!(old, WorkerReport::Activity(_)),
        WorkerReport::Completed(_) => matches!(
            old,
            WorkerReport::Completed(_) | WorkerReport::Progress { .. } | WorkerReport::Activity(_)
        ),
        WorkerReport::Error(WorkerError::Exec { .. }) => matches!(
            old,
            WorkerReport::Error(WorkerError::Exec { .. }) | WorkerReport::Progress { .. }
        ),
        WorkerReport::Installed { .. }
        | WorkerReport::Cleared
        | WorkerReport::Error(_)
        | WorkerReport::Progress { .. } => false,
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::MutexExt;

    use crate::core::worker_bridge::report_mailbox::ReportMailbox;

    impl ReportMailbox {
        /// Every report posted so far, superseded or not.
        pub(crate) fn posts(&self) -> usize {
            self.pending.lock_unpoisoned().posts
        }
    }
}
