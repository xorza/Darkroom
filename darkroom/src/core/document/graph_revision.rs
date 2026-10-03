//! Which state of a graph its output types were worked out from.

use std::sync::atomic::{AtomicU64, Ordering};

/// One state of a graph's node set and wiring — what a wildcard output's
/// type is resolved from. Every open document starts at a revision no other
/// has had, and every edit that can retype an output moves its document to a
/// fresh one, so two equal revisions are one graph in one state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GraphRevision(u64);

impl GraphRevision {
    /// A revision no graph has had before.
    pub(crate) fn fresh() -> Self {
        static LAST: AtomicU64 = AtomicU64::new(0);
        // Relaxed: the counter orders nothing else, it only has to be unique.
        Self(LAST.fetch_add(1, Ordering::Relaxed) + 1)
    }
}
