//! Why the runtime host sent a run nothing.

/// A run, an eviction or a flush of a node the compiled program holds no work
/// for: an edit in the same frame removed it, or it is a stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("nothing to run: this node has no compiled work")]
pub(crate) struct NoCompiledWork;
