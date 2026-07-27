//! The frame's pending mutations, each tagged with the graph it commits
//! against.

use crate::core::document::GraphRef;
use crate::core::edit::intent::types::Intent;

/// A frame's queued [`Intent`]s paired with their edit targets.
///
/// Several graph panes can be on screen at once, so an intent only means
/// something alongside the graph it applies to. Rather than widen every
/// widget signature with a `GraphRef`, the sink carries a *current* target:
/// [`Self::for_graph`] sets it for the duration of a closure and everything
/// pushed inside inherits it. A per-pane draw wraps its whole subtree in one
/// call; a whole-scene scan — which resolves its own target from the node it
/// hit — wraps each push instead.
///
/// Document-global intents (`Intent::Dock`, `Intent::RenameGraph`) are
/// peeled off ahead of the scope lookup in `build_step`, so whichever target
/// happens to be current when one is pushed makes no difference.
#[derive(Debug)]
pub(crate) struct Intents {
    items: Vec<(GraphRef, Intent)>,
    /// Target for the pushes that follow. `Main` until a
    /// [`Self::for_graph`] scope says otherwise, which is also the only
    /// sensible target for a document-global intent raised outside any pane
    /// (a menu-bar command, a keyboard chord with no canvas focused).
    target: GraphRef,
}

impl Default for Intents {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            target: GraphRef::Main,
        }
    }
}

impl Intents {
    /// Queue `intent` against the current target.
    pub(crate) fn push(&mut self, intent: Intent) {
        self.items.push((self.target, intent));
    }

    /// Run `body` with `target` as the current one, restoring the previous
    /// target afterwards so nested scopes compose.
    pub(crate) fn for_graph(&mut self, target: GraphRef, body: impl FnOnce(&mut Self)) {
        let previous = std::mem::replace(&mut self.target, target);
        body(self);
        self.target = previous;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Take everything queued, leaving the buffer empty (and its capacity
    /// intact for the next frame).
    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, (GraphRef, Intent)> {
        self.items.drain(..)
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.target = GraphRef::Main;
    }
}

/// So the many `out.extend(..)` sites that yield bare `Intent`s keep
/// working — each item lands on the current target like a `push`.
impl Extend<Intent> for Intents {
    fn extend<T: IntoIterator<Item = Intent>>(&mut self, iter: T) {
        let target = self.target;
        self.items
            .extend(iter.into_iter().map(|intent| (target, intent)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::{GraphId, NodeId};

    fn raise() -> Intent {
        Intent::Raise { key: NodeId::nil() }
    }

    #[test]
    fn pushes_inherit_the_enclosing_scope_and_scopes_nest() {
        let a = GraphRef::Local(GraphId::from_u128(1));
        let b = GraphRef::Local(GraphId::from_u128(2));
        let mut out = Intents::default();

        // Outside any scope everything lands on `Main`.
        out.push(raise());
        out.for_graph(a, |out| {
            out.push(raise());
            // A nested scope wins while it's open...
            out.for_graph(b, |out| out.extend([raise(), raise()]));
            // ...and the outer target is restored on the way out.
            out.push(raise());
        });
        out.push(raise());

        assert_eq!(
            out.drain().map(|(target, _)| target).collect::<Vec<_>>(),
            [GraphRef::Main, a, b, b, a, GraphRef::Main],
        );
        assert!(out.is_empty(), "draining empties the buffer");
    }

    #[test]
    fn clear_resets_the_current_target() {
        // A scope left open by an early return must not leak into the next
        // frame's pushes — `clear` runs at the top of every frame.
        let mut out = Intents {
            target: GraphRef::Local(GraphId::from_u128(3)),
            ..Default::default()
        };
        out.push(raise());
        out.clear();
        out.push(raise());
        assert_eq!(
            out.drain().map(|(target, _)| target).collect::<Vec<_>>(),
            [GraphRef::Main],
        );
    }
}
