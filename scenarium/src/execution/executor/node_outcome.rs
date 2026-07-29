//! The per-node verdict the run loop records, and what it is allowed to say.

use crate::execution::error::RunError;

/// What became of a node this run — the single per-node result map, so the run-time
/// facts can't contradict (a node can't be `Reused` yet carry a run time, or `Ran` yet
/// also flagged errored). Carries its own `RunError`/elapsed, so nothing lives in a side
/// map.
#[derive(Debug, Clone, Default)]
pub(super) enum NodeOutcome {
    /// Not reached this run: skipped for missing inputs, below a cancel, or unscheduled.
    #[default]
    Pending,
    /// Served from a RAM/disk cache under an unchanged digest — counted as cached.
    Reused,
    /// Pruned by the pre-run cut: every consumer that would read this node reused a cache,
    /// so its output is never read and its lambda is skipped. `cached` is whether its output
    /// remains resident; unprobed disk blobs are not runtime cache state.
    Cut { cached: bool },
    /// Its lambda ran and succeeded, taking `secs`.
    Ran { secs: f64 },
    /// Its lambda ran but errored — an invoke failure, or a cancel mid-invoke.
    Failed { secs: f64, error: RunError },
    /// Never ran — an upstream dependency errored, its func has no implementation attached,
    /// or the cached output it was resolved to reuse failed to load.
    Skipped { error: RunError },
}

impl NodeOutcome {
    /// The run error the node carries — a failed run, or a node skipped for an error.
    pub(super) fn error(&self) -> Option<&RunError> {
        match self {
            NodeOutcome::Failed { error, .. } | NodeOutcome::Skipped { error } => Some(error),
            _ => None,
        }
    }
}
