//! The frame's pending mutations, each tagged with the graph it commits
//! against.

use crate::core::document::GraphRef;
use crate::core::edit::intent::types::{DocIntent, Intent};

/// One queued mutation: an [`Intent`] alongside the graph it commits
/// against, or a [`DocIntent`], which commits against no graph at all.
///
/// The pairing is per-item rather than per-queue because several graph
/// panes can be on screen at once — one frame's queue routinely names two.
#[derive(Debug)]
pub(crate) enum Queued {
    Scoped { target: GraphRef, intent: Intent },
    Global(DocIntent),
}

/// A frame's queued mutations, in the order they were raised.
///
/// An [`Intent`] means nothing without the graph it applies to, so the
/// target is an argument of every push rather than state on the sink. Each
/// site names the pane it belongs to from what it already holds:
/// `SceneNode::owner` in a node body,
/// [`Pane::target`](crate::gui::scene::Pane::target) in a
/// per-pane draw, the latched `GraphRef` in a gesture that outlives its
/// frame. A whole-scene scan names each hit's owner as it goes.
///
/// Nothing is ambient, so there is no wrapper to forget and no default
/// target to absorb the mistake: a push that can't name a graph doesn't
/// compile. And a mutation that *has* no graph doesn't have to pretend —
/// it is a [`DocIntent`], and [`Self::push_global`] is the only door it
/// fits through.
#[derive(Debug, Default)]
pub(crate) struct Intents {
    items: Vec<Queued>,
}

impl Intents {
    /// Queue `intent` against `target`.
    pub(crate) fn push(&mut self, target: GraphRef, intent: Intent) {
        self.items.push(Queued::Scoped { target, intent });
    }

    /// Queue every intent `iter` yields against `target`.
    pub(crate) fn extend(&mut self, target: GraphRef, iter: impl IntoIterator<Item = Intent>) {
        self.items.extend(
            iter.into_iter()
                .map(|intent| Queued::Scoped { target, intent }),
        );
    }

    /// Queue a mutation of the document as a whole, raised from chrome that
    /// is not a canvas (a tab strip, a menu command, a keyboard chord).
    pub(crate) fn push_global(&mut self, intent: DocIntent) {
        self.items.push(Queued::Global(intent));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Take everything queued, leaving the buffer empty (and its capacity
    /// intact for the next frame).
    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, Queued> {
        self.items.drain(..)
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::{GraphId, NodeId};

    fn raise() -> Intent {
        Intent::Raise { key: NodeId::nil() }
    }

    /// The queue keeps what each site named, in order — a scoped intent's
    /// own target, and nothing at all for a global one. (That a graph-scoped
    /// intent can't be queued as global, or a global one against a graph, is
    /// the type system's job now, so there is no runtime case to test.)
    #[test]
    fn every_intent_keeps_the_scope_it_was_pushed_with() {
        let a = GraphRef::Local(GraphId::from_u128(1));
        let b = GraphRef::Local(GraphId::from_u128(2));
        let mut out = Intents::default();

        out.push(a, raise());
        // A whole batch lands on the one target it was extended against,
        // interleaving freely with pushes naming another.
        out.extend(b, [raise(), raise()]);
        out.push(a, raise());
        out.push_global(DocIntent::RenameGraph {
            id: GraphId::from_u128(3),
            to: "renamed".into(),
        });

        let scopes: Vec<Option<GraphRef>> = out
            .drain()
            .map(|item| match item {
                Queued::Scoped { target, .. } => Some(target),
                Queued::Global(_) => None,
            })
            .collect();
        assert_eq!(scopes, [Some(a), Some(b), Some(b), Some(a), None]);
        assert!(out.is_empty(), "draining empties the buffer");
    }
}
