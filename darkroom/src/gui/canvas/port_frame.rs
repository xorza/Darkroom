use std::collections::HashMap;

use glam::Vec2;
use palantir::{Rect, Ui};

use crate::gui::canvas::node_ports;
use crate::gui::node::node_widget_id;
use crate::gui::node::port_row::port_circle_wid;
use crate::gui::{PortKind, PortRef};
use crate::scene::Scene;

/// Per-frame snapshot of the four `ResponseState` fields downstream
/// consumers actually read. Built once at the top of
/// [`crate::gui::canvas::GraphUI::frame`] by polling [`Ui::response_for`]
/// on each port's deterministic [`port_circle_wid`]. Sized to the four
/// bytes-and-bits we use (`layout_rect.center()`, `rect`, two edge bools)
/// instead of the full `ResponseState`.
///
/// Ports that haven't recorded yet (first frame after a node spawns)
/// have an entry with `layout_center` / `screen_rect` = `None`. The
/// edge bools default to `false` for them, so `drag_started` / `dragging`
/// queries are correct without a presence check.
#[derive(Default, Debug)]
pub(crate) struct PortFrame {
    map: HashMap<PortRef, PortInfo>,
    /// Per-port intra-node offset (`port_rect.center - node_rect.min`),
    /// kept **across frames and tab switches**. A port's offset is
    /// layout-stable (it only depends on the node's content, not its
    /// position), so when a graph is shown again — e.g. the frame after
    /// switching back to its tab, where none of its widgets recorded
    /// last frame — we still resolve port centers from `node.pos +
    /// cached_offset` and connections draw on that first frame instead
    /// of popping in one frame late. Keyed by the globally-unique
    /// `PortRef`, so it naturally spans every open graph.
    offsets: HashMap<PortRef, Vec2>,
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
    /// from `response.hovered` in `rebuild`; an active connection
    /// drag's snap target gets it forced on via `set_hovered` after
    /// `ConnectionUI::apply` (palantir's drag-capture suppression
    /// otherwise hides the snap target from `response.hovered`).
    hovered: bool,
    /// One-frame edge: pointer-down → drag latched on this port this
    /// frame. Drives connection-drag start detection.
    drag_started: bool,
    /// Continuous: a drag is currently live on this port
    /// (`drag_delta` is `Some` OR `drag_started` fired this frame).
    /// Read on the start port to detect release.
    dragging: bool,
}

impl PortFrame {
    pub(crate) fn rebuild(&mut self, ui: &Ui, scene: &Scene) {
        self.map.clear();
        for n in &scene.nodes {
            // Port offsets within a node are stable; the node's
            // canvas-local position changes when the user drags. Take
            // `port_offset = port_rect.center - node_rect.min` from
            // last frame's layout (same frame for both, so any
            // ancestor-shared canvas-origin term cancels) and combine
            // with this frame's `n.pos` — curves anchor on the moved
            // node's *current* port positions, not last frame's.
            let node_min = ui
                .response_for(node_widget_id(n.id))
                .layout_rect
                .map(|r| r.min);
            for kind in [PortKind::Input, PortKind::Output] {
                for port in node_ports(n, kind) {
                    let r = ui.response_for(port_circle_wid(port));
                    // Fresh offset this frame (both rects recorded last
                    // frame) refreshes the cache; otherwise fall back to
                    // the cached offset so a just-shown graph still
                    // anchors its curves.
                    let fresh_offset = match (r.layout_rect, node_min) {
                        (Some(port_rect), Some(node_min)) => Some(port_rect.center() - node_min),
                        _ => None,
                    };
                    if let Some(offset) = fresh_offset {
                        self.offsets.insert(port, offset);
                    }
                    let layout_center = fresh_offset
                        .or_else(|| self.offsets.get(&port).copied())
                        .map(|offset| n.pos + offset);
                    self.map.insert(
                        port,
                        PortInfo {
                            layout_center,
                            screen_rect: r.rect,
                            hovered: r.hovered,
                            drag_started: r.drag_started(),
                            dragging: r.drag_started() || r.drag_delta().is_some(),
                        },
                    );
                }
            }
        }
        // Drop offset entries for ports no longer present this frame
        // (deleted nodes, closed subgraphs, shrunk port counts) so the
        // cross-frame cache stays bounded by the *live* port set rather
        // than every port ever seen this session. `map` was rebuilt to
        // exactly the live ports above, so it's the live key set.
        self.offsets.retain(|p, _| self.map.contains_key(p));
    }

    /// Canvas-local pre-transform port center. `None` when the port
    /// or its parent node hasn't been measured yet.
    pub(crate) fn center_canvas_local(&self, p: PortRef) -> Option<Vec2> {
        self.map.get(&p)?.layout_center
    }

    /// `true` when `pointer` (screen coords) falls inside this port's
    /// post-transform/clip rect.
    pub(crate) fn contains_pointer(&self, p: PortRef, pointer: Vec2) -> bool {
        self.map
            .get(&p)
            .and_then(|i| i.screen_rect)
            .is_some_and(|r| r.contains(pointer))
    }

    /// `true` on the one-frame edge of a drag-start on this port.
    pub(crate) fn drag_started(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.drag_started)
    }

    /// `true` while a drag started on this port is still live.
    pub(crate) fn dragging(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.dragging)
    }

    /// `true` when the port should paint with its hover color —
    /// `response.hovered` plus any forced-on override.
    pub(crate) fn is_hovered(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.hovered)
    }

    /// Force the hover flag on (idempotent). Called after
    /// `ConnectionUI::apply` for the active snap target so it lights
    /// up even though palantir's drag-capture suppression hides it
    /// from `response.hovered`.
    pub(crate) fn set_hovered(&mut self, p: PortRef) {
        if let Some(info) = self.map.get_mut(&p) {
            info.hovered = true;
        }
    }
}
