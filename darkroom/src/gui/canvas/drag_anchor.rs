//! The group-drag gesture shared by a node-body drag
//! ([`crate::gui::node::NodeUI`]) and a pin-widget drag
//! ([`crate::gui::canvas::pin_ui::PinUi`]): whichever member the pointer
//! latched drags its whole group (every other selected node and pin)
//! alongside it, as one coalesced `Intent::MoveSelection` per frame.
//!
//! Each caller owns the hit-testing that decides *what* got grabbed — a node
//! body or title, a pin's port circle or preview card — and hands the result
//! to [`GroupDrag::latch`]. Everything after that is identical for both, so
//! it lives here in [`GroupDrag::advance`] rather than being written twice.

use glam::Vec2;
use palantir::{Ui, WidgetId};

use crate::core::document::{GraphRef, ItemRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::scene::{GraphScene, Scene, selection_holds};

/// One in-flight group drag, or none.
///
/// [`Self::advance`] is the whole per-frame lifecycle: the grabbed item's
/// node left the scene, a fresh gesture relatched on the same widget, the
/// drag released, or it moved and commits. Every committed position is
/// `start + drag_delta` against the snapshot taken at latch — not a running
/// integration over the moving widget — so a dropped frame can't accumulate
/// drift.
#[derive(Default, Debug)]
pub(crate) struct GroupDrag {
    anchor: Option<Anchor>,
}

/// The latched drag: what was grabbed, where every moving member started,
/// and whose response drives it.
#[derive(Debug)]
struct Anchor {
    /// The member the pointer grabbed. Names the drag in the emitted intent
    /// (so the edit layer knows which item the user is actually holding),
    /// and its [`ItemRef::owner`] is the node [`GroupDrag::advance`] checks
    /// against the scene.
    grabbed: ItemRef,
    /// The graph pane the drag latched on. Several are on screen, and the
    /// gesture outlives the frame that started it, so the target travels
    /// with the anchor rather than being re-derived from whatever pane the
    /// pointer has since wandered over.
    target: GraphRef,
    /// Every member moving with this drag — node bodies and pin previews
    /// mixed — and its position at drag start: the whole selection when the
    /// grabbed member was already selected, else just the grabbed one.
    start_positions: Vec<(ItemRef, Vec2)>,
    /// The widget whose drag delta drives the gesture, captured at latch so
    /// later frames can `ui.response_for(widget_id)` without the caller
    /// having to remember which of its several grab targets started it.
    widget_id: WidgetId,
}

impl GroupDrag {
    /// Start (or replace) the gesture. `start_positions` includes the
    /// grabbed member itself.
    pub(crate) fn latch(
        &mut self,
        grabbed: ItemRef,
        target: GraphRef,
        start_positions: Vec<(ItemRef, Vec2)>,
        widget_id: WidgetId,
    ) {
        self.anchor = Some(Anchor {
            grabbed,
            target,
            start_positions,
            widget_id,
        });
    }

    /// Drop the gesture when the node owning the grabbed member has left the
    /// scene — a mid-drag delete (breaker swipe, undo). Left in place the
    /// anchor would emit a `MoveSelection` against a missing node, which
    /// panics in `build_step`, and could fire again if a fresh node reused
    /// the id.
    pub(crate) fn drop_if_owner_gone(&mut self, scene: &Scene) {
        let gone = self
            .anchor
            .as_ref()
            .is_some_and(|a| !scene.nodes.contains_key(&a.grabbed.owner()));
        if gone {
            self.anchor = None;
        }
    }

    /// Advance one frame, pushing this frame's `Intent::MoveSelection` when
    /// the drag is still held, and reporting whether it is. A caller that
    /// also latches fresh drags skips its own scan while this returns
    /// `true` — the gesture already owns the frame.
    ///
    /// Runs pre-record, so the move lands in `Document` (and is mirrored
    /// into `Scene`) before the pass that draws the moved items: they paint
    /// at the cursor in Pass A with no relayout retry.
    pub(crate) fn advance(&mut self, ui: &Ui, scene: &Scene, out: &mut Intents) -> bool {
        self.drop_if_owner_gone(scene);
        // Copy the ids out and drop the borrow, so the branches below can
        // clear the slot without cloning `start_positions` — only the
        // success path reads it, and that path never clears.
        let Some((widget_id, target)) = self.anchor.as_ref().map(|a| (a.widget_id, a.target))
        else {
            return false;
        };
        let resp = ui.response_for(widget_id);
        // `drag_started` on a still-active anchor means a *new* gesture just
        // latched on the same widget. Emitting with the stale start
        // positions would snap the group back to the previous gesture's
        // start point; the caller's latch scan picks the new one up instead.
        if resp.left.drag.started() {
            self.anchor = None;
            return false;
        }
        // No delta means the drag isn't latched anymore — release, or the
        // pointer left the surface.
        let Some(delta) = resp.left.drag.delta() else {
            self.anchor = None;
            return false;
        };
        // Palantir reports drag deltas in the widget's pre-transform frame,
        // which is the same canvas-world space item positions live in.
        let move_selection = self.anchor.as_ref().unwrap().resolve(delta);
        out.for_graph(target, |out| out.push(move_selection));
        true
    }
}

impl Anchor {
    /// This frame's `Intent::MoveSelection`: every member's latch-time start
    /// plus Palantir's pre-transform drag `offset`.
    fn resolve(&self, offset: Vec2) -> Intent {
        Intent::MoveSelection {
            grabbed: self.grabbed,
            moves: self
                .start_positions
                .iter()
                .map(|(key, start)| (*key, *start + offset))
                .collect(),
        }
    }
}

/// Resolve the current selection into [`GroupDrag::latch`]'s
/// `start_positions` for a drag that grabbed an already-selected member —
/// shared by both callers, so the group moves the same way regardless of
/// which kind of member's press started it.
pub(crate) fn selected_group_positions(
    graph: GraphScene<'_>,
    selected: &[ItemRef],
) -> Vec<(ItemRef, Vec2)> {
    let holds = |key: ItemRef| selection_holds(selected, key);
    let mut positions: Vec<(ItemRef, Vec2)> = graph
        .nodes()
        .filter(|n| holds(ItemRef::Node(n.id)))
        .map(|n| (ItemRef::Node(n.id), n.pos))
        .collect();
    for pin in graph.pinned_outputs() {
        let key = ItemRef::Pin(pin.port);
        if holds(key) {
            positions.push((key, pin.pos));
        }
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::{NodeId, OutputPort};

    use crate::gui::scene::internals::scene_node_stub;

    fn wid() -> WidgetId {
        WidgetId::from_hash("group_drag_test_widget")
    }

    /// A scene holding just `id`, so a drag grabbing something on it passes
    /// the owner check.
    fn scene_with(ui: &mut Ui, id: NodeId) -> Scene {
        Scene::with_nodes([scene_node_stub(ui, id, Vec2::ZERO)])
    }

    #[test]
    fn resolve_offsets_every_member_from_its_own_latch_start() {
        // A mixed group — the grabbed node plus a pin hanging off a
        // different node — moves rigidly: each member commits to its *own*
        // latch-time start plus the current offset. Measuring from the
        // latch (rather than integrating frame to frame) is what keeps a
        // dropped or coalesced frame from accumulating drift.
        let grabbed_node = NodeId::unique();
        let other_pin = ItemRef::Pin(OutputPort::new(NodeId::unique(), 2));
        let anchor = Anchor {
            grabbed: ItemRef::Node(grabbed_node),
            target: GraphRef::Main,
            start_positions: vec![
                (ItemRef::Node(grabbed_node), Vec2::new(10.0, 20.0)),
                (other_pin, Vec2::new(-5.0, 100.0)),
            ],
            widget_id: wid(),
        };

        let Intent::MoveSelection { grabbed, moves } = anchor.resolve(Vec2::new(3.0, -7.0)) else {
            panic!("a group drag commits a MoveSelection");
        };
        assert_eq!(grabbed, ItemRef::Node(grabbed_node));
        // Hand-computed: (10,20)+(3,-7) = (13,13); (-5,100)+(3,-7) = (-2,93).
        assert_eq!(
            moves,
            vec![
                (ItemRef::Node(grabbed_node), Vec2::new(13.0, 13.0)),
                (other_pin, Vec2::new(-2.0, 93.0)),
            ],
        );

        // A later, larger offset still measures from those same starts, not
        // from where the previous frame left the items: (10,20)+(6,-14) =
        // (16,6); (-5,100)+(6,-14) = (1,86).
        let Intent::MoveSelection { moves, .. } = anchor.resolve(Vec2::new(6.0, -14.0)) else {
            panic!("a group drag commits a MoveSelection");
        };
        assert_eq!(
            moves,
            vec![
                (ItemRef::Node(grabbed_node), Vec2::new(16.0, 6.0)),
                (other_pin, Vec2::new(1.0, 86.0)),
            ],
        );
    }

    #[test]
    fn advance_drops_a_stale_anchor_without_emitting() {
        // Undo or a breaker swipe can delete the dragged item's node
        // mid-gesture. The anchor has to let go: a `MoveSelection` naming a
        // node that left the scene panics in `build_step`. The grabbed key
        // here is a *pin*, so this also covers `ItemRef::owner` reaching
        // through to the node a pin hangs off.
        let mut ui = Ui::default();
        let survivor = NodeId::unique();
        let scene = scene_with(&mut ui, survivor);

        let mut drag = GroupDrag::default();
        let gone = ItemRef::Pin(OutputPort::new(NodeId::unique(), 0));
        drag.latch(gone, GraphRef::Main, vec![(gone, Vec2::ZERO)], wid());

        let mut out = Intents::default();
        assert!(!drag.advance(&ui, &scene, &mut out), "the drag is over");
        assert!(out.is_empty(), "a stale anchor emits nothing");
        assert!(drag.anchor.is_none(), "and drops itself");
    }

    #[test]
    fn advance_releases_when_the_widget_stops_reporting_a_delta() {
        // A bare `Ui` reports no drag on the anchor's widget, which is the
        // release edge: the gesture ends without emitting, so the next press
        // latches fresh instead of resuming this one's start positions.
        let mut ui = Ui::default();
        let id = NodeId::unique();
        let scene = scene_with(&mut ui, id);

        let mut drag = GroupDrag::default();
        let key = ItemRef::Node(id);
        drag.latch(key, GraphRef::Main, vec![(key, Vec2::new(4.0, 4.0))], wid());
        assert!(drag.anchor.is_some(), "latched");

        let mut out = Intents::default();
        assert!(!drag.advance(&ui, &scene, &mut out));
        assert!(out.is_empty(), "a release commits nothing of its own");
        assert!(drag.anchor.is_none(), "the slot is free for the next press");
    }
}
