//! [`StackingProgress`]: one progress report of a stacking run, and the stage it belongs to.

/// Progress information for stacking operations.
#[derive(Debug, Clone)]
pub struct StackingProgress {
    /// Units of the stage's work done so far, from 1; the stage's last report has `total`.
    pub current: usize,
    /// Units of work in the stage.
    pub total: usize,
    /// Description of current operation.
    pub stage: StackingStage,
}

/// The pass a [`StackingProgress`] report belongs to.
///
/// One variant per pass that walks a countable set, so `current`/`total` mean one thing within a
/// stage and a stage change is a real change of work. Each stage reports once per unit it
/// completes, in order, and a run emits only the stages its route uses — but which stages those are
/// follows from the work asked for, not from which function was called: both front ends report
/// `Preparing` and `Registering`, a statistical combine reports `Loading` and `Combining` where a
/// drizzle reports `Drizzling`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingStage {
    /// Turning inputs into frames with detected stars: decoding, calibrating and demosaicing where
    /// the input is raw, and detecting each frame's stars either way. Counted in frames.
    ///
    /// One stage rather than one per activity because it is one pass: the raw path detects while
    /// the decoded frame is still in hand, so a frame is reported once whichever route it took.
    Preparing,
    /// Registering each frame against the reference and warping it into place. Counted in
    /// frames, and the reference itself is not among them.
    Registering,
    /// Reading frames into the combine's cache, spilling them to disk on the streaming tier.
    /// Counted in frames.
    Loading,
    /// Walking the output in row chunks and reducing the frames into it. Counted in
    /// chunk-channel pairs, not frames.
    Combining,
    /// Accumulating frames into the drizzle grid. Counted in frames.
    Drizzling,
}
