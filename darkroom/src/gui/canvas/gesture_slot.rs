//! Which pane owns a gesture.
//!
//! One `GraphUI` drives every visible canvas, so a controller's per-pane
//! pass runs N times for one gesture and has to know whether this is *its*
//! pane. The answer is load-bearing, not cosmetic: a scribble's polyline
//! lives in its own graph's pre-transform coordinates, a rubber band would
//! paint on both canvases, and a wire would snap to a neighbouring graph's
//! ports.
//!
//! [`PaneSlot`] is where that lives. The pane rides on the *slot*, not on
//! the state it holds — "at most one of these, and it belongs to one pane"
//! is a single fact, and keeping it in one place is what makes
//! [`PaneSlot::latch`] the only site that can get it wrong.

use crate::core::document::GraphRef;

/// At most one gesture, together with the pane it latched on.
///
/// Replaces the `Option<S>` each controller used to hold beside a
/// `GraphRef` — stored on the state by some, re-derived per read from the
/// node the gesture hangs off by others. Both spellings were correct and
/// neither was checkable; here the pane cannot be absent, stale, or
/// disagree with the gesture, because there is nowhere for it to live
/// apart from it.
#[derive(Debug)]
pub(super) struct PaneSlot<S> {
    held: Option<Held<S>>,
}

/// A gesture and the pane it latched on, handed back together by
/// [`PaneSlot::take_held`] so a caller that needs both cannot ask for one
/// and then re-ask for the other against an impossible `None`.
#[derive(Debug)]
pub(super) struct Held<S> {
    pub(super) graph: GraphRef,
    pub(super) state: S,
}

/// Hand-written: `derive` would demand `S: Default`, and an empty slot
/// holds no state to default.
impl<S> Default for PaneSlot<S> {
    fn default() -> Self {
        Self { held: None }
    }
}

impl<S> PaneSlot<S> {
    /// Hold `state` on `graph`'s pane, replacing anything already held.
    ///
    /// The one place a gesture and a pane are paired. A caller that
    /// resolves the pane from elsewhere — the node a wire grew out of,
    /// say — resolves it *here*, once, instead of at every later read.
    pub(super) fn latch(&mut self, graph: GraphRef, state: S) {
        self.held = Some(Held { graph, state });
    }

    /// Drop whatever is held, from any pane.
    pub(super) fn clear(&mut self) {
        self.held = None;
    }

    /// Whether nothing is held, on any pane — the guard a latch opens
    /// with, since a controller only starts a gesture when it has none.
    pub(super) fn is_idle(&self) -> bool {
        self.held.is_none()
    }

    /// Whether *another* pane holds one — the early return a per-pane pass
    /// opens with.
    ///
    /// Distinct from [`Self::get`] answering `None`, which also covers an
    /// empty slot: a pass that bailed on an empty slot would never latch a
    /// new gesture, and the canvas would go inert.
    pub(super) fn elsewhere(&self, target: GraphRef) -> bool {
        self.held.as_ref().is_some_and(|held| held.graph != target)
    }

    /// The held gesture, if `target` is the pane it latched on.
    pub(super) fn get(&self, target: GraphRef) -> Option<&S> {
        self.held
            .as_ref()
            .filter(|held| held.graph == target)
            .map(|held| &held.state)
    }

    /// Same, to advance it in place.
    pub(super) fn get_mut(&mut self, target: GraphRef) -> Option<&mut S> {
        self.held
            .as_mut()
            .filter(|held| held.graph == target)
            .map(|held| &mut held.state)
    }

    /// Whatever is held, **whichever pane holds it**.
    ///
    /// For the once-per-frame work that is pane-agnostic by nature —
    /// baking a snap target into `CanvasGeometry`, whose flags are keyed
    /// by document-unique glyph ids. Everything drawn or edited per pane
    /// wants [`Self::get`] instead.
    pub(super) fn held(&self) -> Option<&S> {
        self.held.as_ref().map(|held| &held.state)
    }

    /// Take the held gesture **and its pane**, whichever pane that is.
    ///
    /// The shape a whole-scene state machine wants: take, advance,
    /// [`Self::latch`] it back — and simply not re-latching is how it
    /// ends. Handing both back at once is what keeps the pane from being
    /// asked for separately and then re-asked against an `Option` that
    /// cannot be `None`.
    pub(super) fn take_held(&mut self) -> Option<Held<S>> {
        self.held.take()
    }

    /// Take the held gesture out, if `target` owns it — the per-pane
    /// counterpart of [`Self::take_held`], for a slot every pane asks and
    /// only the owner may consume.
    pub(super) fn take(&mut self, target: GraphRef) -> Option<S> {
        let held = self.held.take()?;
        if held.graph == target {
            return Some(held.state);
        }
        self.held = Some(held);
        None
    }
}

#[cfg(test)]
mod tests {
    use scenarium::GraphId;

    use super::*;

    /// `elsewhere` must tell "nothing in flight" apart from "in flight,
    /// elsewhere", and a refused `take` must leave the gesture held.
    ///
    /// Every per-pane pass opens with `elsewhere`, so answering `true` on
    /// an empty slot would stop *every* pane from latching a new gesture —
    /// the canvas would go inert. Answering `false` when a neighbour holds
    /// one is the other half: that pane would advance a scribble in its
    /// own coordinates.
    #[test]
    fn an_empty_slot_is_not_elsewhere_and_a_neighbours_is() {
        let other = GraphRef::Local(GraphId::from_u128(1));
        let mut slot: PaneSlot<u8> = PaneSlot::default();

        assert!(
            !slot.elsewhere(GraphRef::Main),
            "an idle slot blocks nobody"
        );
        assert_eq!(slot.get(GraphRef::Main), None, "and offers nothing");
        assert!(slot.is_idle());

        slot.latch(GraphRef::Main, 7);
        assert!(!slot.elsewhere(GraphRef::Main), "the owner may advance it");
        assert_eq!(slot.get(GraphRef::Main), Some(&7));
        assert!(slot.elsewhere(other), "every other pane stands down");
        assert_eq!(slot.get(other), None, "and cannot read it either");

        assert_eq!(slot.take(other), None, "a foreign take is refused");
        assert!(!slot.is_idle(), "and leaves it held");
        assert_eq!(slot.take(GraphRef::Main), Some(7));
        assert!(slot.is_idle(), "the owner's take empties it");

        // `take_held` hands back both halves, so no caller has to ask for
        // the pane and then re-ask for a state that cannot be missing.
        slot.latch(other, 9);
        let held = slot.take_held().expect("just latched");
        assert_eq!(held.graph, other);
        assert_eq!(held.state, 9);
        assert!(slot.is_idle());
    }
}
