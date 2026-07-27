//! The frame's pending mutations, each tagged with the graph it commits
//! against.

use crate::core::document::GraphRef;
use crate::core::edit::intent::types::Intent;

/// A frame's queued [`Intent`]s paired with their edit targets.
///
/// Several graph panes can be on screen at once, so an intent only means
/// something alongside the graph it applies to — which is why the target is
/// an argument of every push rather than state on the sink. Each site names
/// the pane it belongs to from what it already holds: `SceneNode::owner` in
/// a node body, [`GraphScene::target`](crate::gui::scene::GraphScene::target)
/// in a per-pane draw, the latched `GraphRef` in a gesture that outlives its
/// frame. A whole-scene scan names each hit's owner as it goes.
///
/// Nothing is ambient, so there is no wrapper to forget and no default
/// target to absorb the mistake: a push that can't name a graph doesn't
/// compile.
#[derive(Debug, Default)]
pub(crate) struct Intents {
    items: Vec<(GraphRef, Intent)>,
}

impl Intents {
    /// Queue `intent` against `target`.
    pub(crate) fn push(&mut self, target: GraphRef, intent: Intent) {
        self.items.push((target, intent));
    }

    /// Queue every intent `iter` yields against `target`.
    pub(crate) fn extend(&mut self, target: GraphRef, iter: impl IntoIterator<Item = Intent>) {
        self.items
            .extend(iter.into_iter().map(|intent| (target, intent)));
    }

    /// Queue a document-global intent — one no graph owns, raised from
    /// chrome that is not a canvas (a tab strip, a menu command, a keyboard
    /// chord). `build_step` peels these off ahead of the scope lookup, so
    /// the target they queue under is never read; `Main` keeps the queue one
    /// uniform shape.
    pub(crate) fn push_global(&mut self, intent: Intent) {
        debug_assert!(
            intent.is_global(),
            "{intent:?} commits against a graph — push it against one",
        );
        self.items.push((GraphRef::Main, intent));
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
    fn every_intent_keeps_the_target_it_was_pushed_against() {
        let a = GraphRef::Local(GraphId::from_u128(1));
        let b = GraphRef::Local(GraphId::from_u128(2));
        let mut out = Intents::default();

        out.push(a, raise());
        // A whole batch lands on the one target it was extended against,
        // interleaving freely with pushes naming another.
        out.extend(b, [raise(), raise()]);
        out.push(a, raise());
        // Document-global intents queue without naming a graph.
        out.push_global(Intent::RenameGraph {
            id: GraphId::from_u128(3),
            to: "renamed".into(),
        });

        assert_eq!(
            out.drain().map(|(target, _)| target).collect::<Vec<_>>(),
            [a, b, b, a, GraphRef::Main],
        );
        assert!(out.is_empty(), "draining empties the buffer");
    }

    #[test]
    #[should_panic(expected = "commits against a graph")]
    fn a_graph_scoped_intent_cannot_slip_through_the_global_door() {
        Intents::default().push_global(raise());
    }
}
