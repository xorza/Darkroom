//! [`DockPath`]: a split's address, packed into one byte.

use serde::{Deserialize, Serialize};

/// A split's address: the turns taken from the root, packed into one
/// byte — a leading sentinel bit, then one bit per level (`0` = first
/// child, `1` = second). The root split is the bare sentinel. One
/// `Copy` byte instead of a `Vec<bool>`, with capacity for 7 levels —
/// [`MAX_SPLIT_DEPTH`](crate::core::document::dock::MAX_SPLIT_DEPTH) keeps real trees well inside that.
///
/// Like any address into the layout it's only stable between
/// structural changes; a stale path that no longer lands on a split is
/// ignored by the ops it feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct DockPath(u8);

impl DockPath {
    /// The root node's address (the empty path).
    pub(crate) const ROOT: DockPath = DockPath(1);

    /// The address of `self`'s first (left/top) child.
    pub(crate) fn first(self) -> DockPath {
        self.child(false)
    }

    /// The address of `self`'s second (right/bottom) child.
    pub(crate) fn second(self) -> DockPath {
        self.child(true)
    }

    fn child(self, second: bool) -> DockPath {
        assert!(
            self.0 < 0x80,
            "dock path capacity (7 levels) exceeded — MAX_SPLIT_DEPTH should stop far earlier"
        );
        DockPath((self.0 << 1) | second as u8)
    }

    /// Whether the byte carries no sentinel bit — a corrupt address rather
    /// than the root, reachable only through serde.
    pub(super) fn is_corrupt(self) -> bool {
        self.0 == 0
    }

    /// Turns from the root, in root→leaf order. Saturating so the
    /// invalid sentinel-less `0` byte (reachable only through serde)
    /// yields no turns instead of underflowing.
    pub(super) fn directions(self) -> impl Iterator<Item = bool> {
        let depth = 7u32.saturating_sub(self.0.leading_zeros());
        (0..depth).rev().map(move |i| (self.0 >> i) & 1 == 1)
    }
}

impl Default for DockPath {
    fn default() -> Self {
        Self::ROOT
    }
}
