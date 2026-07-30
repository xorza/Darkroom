//! At most one canvas gesture at a time.
//!
//! [`GestureSlot`] is where that lives: an `Option` wearing the gesture
//! vocabulary the controllers read in — latch, hold, take.

/// At most one gesture.
#[derive(Debug)]
pub(super) struct GestureSlot<S> {
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
    pub(super) fn latch(&mut self, state: S) {
        self.held = Some(state);
    }

    pub(super) fn clear(&mut self) {
        self.held = None;
    }

    /// Whether nothing is held — the guard a latch opens with, since a
    /// controller only starts a gesture when it has none.
    pub(super) fn is_idle(&self) -> bool {
        self.held.is_none()
    }

    /// The held gesture.
    pub(super) fn get(&self) -> Option<&S> {
        self.held.as_ref()
    }

    /// Same, to advance it in place.
    pub(super) fn get_mut(&mut self) -> Option<&mut S> {
        self.held.as_mut()
    }

    /// Take the held gesture out. The shape a state machine wants: take,
    /// advance, [`Self::latch`] it back — and simply not re-latching is how
    /// it ends.
    pub(super) fn take(&mut self) -> Option<S> {
        self.held.take()
    }
}
