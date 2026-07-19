//! The group-drag anchor shape shared by a node-body drag
//! ([`crate::gui::node::NodeUI`]'s `DragAnchor`) and a pin-widget drag
//! ([`crate::gui::canvas::pin_ui::PinUi`]'s `PinDragAnchor`): whichever
//! member the pointer latched drags its whole group (every other selected
//! node and pin) alongside it. Every later frame's committed position is
//! `start + drag_delta`, not a running integration over the moving widget,
//! so both draw a fresh `Intent::MoveSelection` off the same start snapshot
//! each frame the drag is held.

use std::collections::BTreeSet;

use glam::Vec2;

use crate::core::document::ItemRef;
use crate::core::edit::intent::types::Intent;
use crate::gui::scene::Scene;

/// `K` is the grabbed member's own key — a `NodeId` for a node body, an
/// `OutputPort` for a pin widget — kept generic so each caller's `apply`/
/// `prepass` can pattern-match its own domain type without downcasting a
/// shared enum.
#[derive(Clone, Debug)]
pub(crate) struct GroupDragAnchor<K> {
    pub(crate) key: K,
    /// Every member moving with this drag — node bodies and pin previews
    /// mixed — and its position at drag start: the whole selection when
    /// the grabbed member was already selected, else just the grabbed one.
    pub(crate) start_positions: Vec<(ItemRef, Vec2)>,
    /// Captured at latch so later frames can `ui.response_for(widget_id)`
    /// directly: the node body/title (or port circle / preview widget for
    /// a pin) whose drag delta drives this anchor.
    pub(crate) widget_id: aperture::WidgetId,
}

impl<K> GroupDragAnchor<K> {
    /// Rebuild this frame's `Intent::MoveSelection` from a zoom-adjusted
    /// `offset` — the "start + offset" shape both callers' resolve step
    /// shares, regardless of what kind of member grabbed the drag.
    /// `grabbed` names the member for the intent's own record (the caller
    /// already knows which `ItemRef` variant wraps `self.key`).
    pub(crate) fn resolve(&self, offset: Vec2, grabbed: ItemRef) -> Intent {
        let moves = self
            .start_positions
            .iter()
            .map(|(key, start)| (*key, *start + offset))
            .collect();
        Intent::MoveSelection { grabbed, moves }
    }
}

/// Resolve the current selection into a [`GroupDragAnchor`]'s
/// `start_positions` for a group drag latched by grabbing either kind of
/// member — shared by `NodeUI`'s node-body drag and `PinUi`'s pin-widget
/// drag, so both produce the same [`Intent::MoveSelection`] group
/// regardless of which member's press started it.
pub(crate) fn selected_group_positions(
    scene: &Scene,
    selected: &BTreeSet<ItemRef>,
) -> Vec<(ItemRef, Vec2)> {
    let mut positions: Vec<(ItemRef, Vec2)> = scene
        .nodes
        .values()
        .filter(|n| selected.contains(&ItemRef::Node(n.id)))
        .map(|n| (ItemRef::Node(n.id), n.pos))
        .collect();
    for pin in scene.pinned_outputs() {
        let key = ItemRef::Pin(pin.port);
        if selected.contains(&key) {
            positions.push((key, pin.pos));
        }
    }
    positions
}
