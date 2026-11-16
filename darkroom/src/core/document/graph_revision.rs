//! Which state of a graph's node set a cache was swept at.

/// One state of a graph's node set. Every edit that adds or removes a node of a graph moves that
/// graph to its next revision, so two equal revisions of one graph hold the same nodes.
///
/// Counted per graph of a document, so two graphs or two documents can share a revision: a cache
/// that keeps one must not outlive its graph's pane. The window's node-cache sweeps live in the
/// `MainWindow` that `Session::new` builds afresh with each document.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GraphRevision(u64);

impl GraphRevision {
    /// The revision after this one.
    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}
