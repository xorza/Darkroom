//! [`PortLayer`]: one key domain's glyph snapshot, live for a frame and anchored across frames.

use std::collections::HashMap;

use glam::Vec2;
use palantir::{Rect, ResponseState};

use scenarium::NodeId;

use crate::core::document::graph_view::GraphView;
use crate::gui::node_key::NodeKey;

/// One key-domain's port snapshot, split into two tiers by lifetime:
///
/// - `live` is cleared and rebuilt every frame from last frame's responses.
/// - `offsets` (per-widget intra-node offset, `widget_rect.center -
///   node_rect.min`) is kept **across frames and tab switches**. An offset
///   is layout-stable (it depends only on the node's content, not its
///   position), so when a graph is shown again — e.g. the frame after
///   switching back to its tab, where none of its widgets recorded last
///   frame — centers still resolve from `node.pos + cached_offset` and
///   wires draw on that first frame instead of popping in one frame
///   late. Keyed by the globally-unique domain key, so it spans every open
///   graph; on document reload the whole `GraphUi` (and this cache) is dropped.
#[derive(Debug)]
pub(crate) struct PortLayer<K> {
    live: HashMap<K, PortInfo>,
    offsets: HashMap<K, Vec2>,
    /// The glyph a drag started on this frame, noted as the walk records it.
    /// One press starts at most one drag, so the controllers latching off it
    /// read it here instead of probing every candidate glyph.
    started_drag: Option<K>,
}

/// Where a node's body measured last frame, and where it sits this frame: the
/// two ends a glyph's offset spans.
#[derive(Clone, Copy, Debug)]
pub(super) struct NodePlace {
    /// The body's layout-rect corner, from last frame's layout.
    pub(super) layout_min: Vec2,
    pub(super) pos: Vec2,
}

impl<K> Default for PortLayer<K> {
    fn default() -> Self {
        Self {
            live: HashMap::new(),
            offsets: HashMap::new(),
            started_drag: None,
        }
    }
}

impl<K: NodeKey> PortLayer<K> {
    /// Snapshot one widget's response `r` into `live`: refresh the
    /// intra-node offset from this frame's rects when both recorded, else fall
    /// back to the cached offset so a just-shown graph still anchors. The
    /// center is the node's position plus the offset, so a moved node's glyph
    /// tracks its current position.
    pub(super) fn record(&mut self, key: K, r: ResponseState, node: NodePlace) {
        let fresh_offset = r.layout_rect.map(|rect| rect.center() - node.layout_min);
        if let Some(offset) = fresh_offset {
            self.offsets.insert(key, offset);
        }
        let layout_center = fresh_offset
            .or_else(|| self.offsets.get(&key).copied())
            .map(|offset| node.pos + offset);
        let info = PortInfo {
            layout_center,
            screen_rect: r.rect,
            hovered: r.hovered(),
            drag_started: r.left.drag.started(),
            dragging: r.left.drag.started() || r.left.drag.delta().is_some(),
        };
        if info.drag_started && self.started_drag.is_none() {
            self.started_drag = Some(key);
        }
        self.live.insert(key, info);
    }

    /// Move each center to its node's position in `view`, which the drain between the prepass
    /// and the record moves a dragged node to; the interaction state stays as the prepass read it.
    #[expect(
        clippy::iter_over_hash_type,
        reason = "each center moves on its own, so the order changes nothing"
    )]
    pub(super) fn reposition(&mut self, view: &GraphView) {
        for (key, info) in &mut self.live {
            if let (Some(offset), Some(placement)) =
                (self.offsets.get(key), view.node_placements.get(&key.node()))
            {
                info.layout_center = Some(placement.pos + *offset);
            }
        }
    }

    /// Drop the offsets of every glyph on a node `keep` refuses; see
    /// [`CanvasGeometry::retain_nodes`](crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry::retain_nodes).
    pub(super) fn retain_nodes(&mut self, keep: impl Fn(NodeId) -> bool) {
        self.offsets.retain(|key, _| keep(key.node()));
    }

    /// Forget last frame's snapshots, keeping the offsets and the capacity.
    pub(super) fn begin_frame(&mut self) {
        self.live.clear();
        self.started_drag = None;
    }

    /// The entry for a glyph whose node didn't record: its center from the
    /// persistent offset against this frame's `node_pos`, and no interaction
    /// state at all. Equivalent to [`Self::record`] with a default response —
    /// which is exactly what `response_for` would have returned — without
    /// asking for one.
    pub(super) fn replay(&mut self, key: K, node_pos: Vec2) {
        let Some(offset) = self.offsets.get(&key).copied() else {
            return;
        };
        self.live.insert(
            key,
            PortInfo {
                layout_center: Some(node_pos + offset),
                screen_rect: None,
                hovered: false,
                drag_started: false,
                dragging: false,
            },
        );
    }

    /// Canvas-local pre-transform center, or `None` when the widget or its
    /// parent node hasn't measured yet. `pub(crate)` for the port menu's "Add
    /// preview", which spawns its node off the port circle it was opened on
    /// (`crate::gui::pane::graph_ui::node_ui::port_row`); everything else that reads a center is
    /// inside the canvas.
    pub(crate) fn center(&self, key: K) -> Option<Vec2> {
        self.live.get(&key)?.layout_center
    }

    /// First key in `keys` whose widget contains `pointer` (screen coords),
    /// or `None` — the "which glyph is the in-flight wire hovering" scan
    /// every snap-target search needs, differing only in the candidate
    /// sequence it feeds in and whatever acceptance test it then applies to
    /// the winner. Geometrically at most one glyph sits under the pointer, so
    /// a rejected winner means "no snap", not "keep looking". Sibling of
    /// [`Self::started_drag`].
    ///
    /// Tests the post-transform/clip rect, so it sees through palantir's
    /// drag-capture hover suppression.
    pub(crate) fn first_containing(
        &self,
        pointer: Vec2,
        mut keys: impl Iterator<Item = K>,
    ) -> Option<K> {
        keys.find(|k| {
            self.live
                .get(k)
                .and_then(|i| i.screen_rect)
                .is_some_and(|r| r.contains(pointer))
        })
    }

    /// The glyph a drag started on this frame, or `None` — what every
    /// drag-source controller (wire, preview spawn, event wire) latches
    /// on, each keeping only the kinds of glyph it starts from.
    pub(crate) const fn started_drag(&self) -> Option<K> {
        self.started_drag
    }

    /// `true` while a drag started on this widget is still live.
    pub(crate) fn dragging(&self, key: K) -> bool {
        self.live.get(&key).is_some_and(|i| i.dragging)
    }

    /// `true` when this widget should paint with its hover color.
    pub(crate) fn is_hovered(&self, key: K) -> bool {
        self.live.get(&key).is_some_and(|i| i.hovered)
    }

    /// Force the hover flag on (idempotent) — the active drag's snap target,
    /// which palantir's drag-capture suppression hides from `response.hovered`.
    pub(crate) fn set_hovered(&mut self, key: K) {
        if let Some(info) = self.live.get_mut(&key) {
            info.hovered = true;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PortInfo {
    /// Port-circle center in canvas-local (inner-canvas pre-transform)
    /// coords. Computed as `node.pos + port_offset_within_node` so a
    /// just-moved node's curves anchor on this frame's port positions
    /// instead of last frame's stale `response.layout_rect`. `None`
    /// when either the port or its parent node hasn't measured yet.
    layout_center: Option<Vec2>,
    /// Post-transform/clip screen rect for pointer hit-test (snap).
    /// Bypasses palantir's drag-capture hover suppression by reading
    /// geometry directly.
    screen_rect: Option<Rect>,
    /// `true` when the port should paint with its hover color. Filled
    /// from `response.hovered` in `rebuild`; an active wire
    /// drag's snap target gets it forced on via `set_hovered` after
    /// `WireUi::prepass` (palantir's drag-capture suppression
    /// otherwise hides the snap target from `response.hovered`).
    hovered: bool,
    /// One-frame edge: pointer-down → drag latched on this port this
    /// frame. Drives wire-drag start detection.
    drag_started: bool,
    /// Continuous: a drag is currently live on this port
    /// (`drag_delta` is `Some` OR `drag_started` fired this frame).
    /// Read on the start port to detect release.
    dragging: bool,
}
