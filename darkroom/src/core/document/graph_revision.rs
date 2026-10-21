//! Which state of a graph its output types were worked out from.

/// One state of a graph's node set and wiring — what a wildcard output's
/// type is resolved from. Every edit that can retype an output moves its
/// document to the next revision, so two equal revisions of one document are
/// one graph in one state.
///
/// Counted per document, so two documents can share a revision: a cache that
/// keeps one must not outlive its document. Both that do — the window's
/// output-type table and its node-cache sweep — live in the `MainWindow` that
/// `Session::new` builds afresh with each document.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GraphRevision(u64);

impl GraphRevision {
    /// The revision after this one.
    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}
