//! At most one gesture at a time.
//!
//! [`GestureSlot`] is where that lives: an `Option` wearing the gesture
//! vocabulary the canvas controllers and the image viewer read in — latch,
//! hold, take.

use glam::Vec2;

/// At most one gesture.
#[derive(Debug)]
pub(crate) struct GestureSlot<S> {
    held: Option<S>,
}

/// Hand-written: `derive` would demand `S: Default`, and an empty slot
/// holds no state to default.
impl<S> Default for GestureSlot<S> {
    fn default() -> Self {
        Self { held: None }
    }
}

impl<S> GestureSlot<S> {
    /// Hold `state`, replacing anything already held.
    pub(crate) fn latch(&mut self, state: S) {
        self.held = Some(state);
    }

    pub(crate) fn clear(&mut self) {
        self.held = None;
    }

    /// Whether nothing is held — the guard a latch opens with, since a
    /// controller only starts a gesture when it has none.
    pub(crate) const fn is_idle(&self) -> bool {
        self.held.is_none()
    }

    /// The held gesture.
    pub(crate) const fn get(&self) -> Option<&S> {
        self.held.as_ref()
    }

    /// Take the held gesture out. The shape a state machine wants: take,
    /// advance, [`Self::latch`] it back — and simply not re-latching is how
    /// it ends.
    pub(crate) const fn take(&mut self) -> Option<S> {
        self.held.take()
    }

    /// Fold a live pan drag into `pan`: the latch's start (read by `start`)
    /// plus `delta` while the drag is held, and a missing delta after a latch
    /// is the release edge that drops it. Returns the latch while it is held;
    /// a call before anything latched does nothing at all. Shared by the
    /// canvas and the image viewer.
    ///
    /// Measured from the latch rather than integrated per frame, so a pan
    /// lands exactly where the pointer says however many frames it took (no
    /// per-frame rounding drift).
    pub(crate) fn fold_pan(
        &mut self,
        start: impl FnOnce(&S) -> Vec2,
        delta: Option<Vec2>,
        pan: &mut Vec2,
    ) -> Option<&S> {
        let Some(delta) = delta else {
            self.clear();
            return None;
        };
        let latched = self.get()?;
        *pan = start(latched) + delta;
        Some(latched)
    }
}
