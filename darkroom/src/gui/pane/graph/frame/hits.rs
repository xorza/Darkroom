//! One sweep of last frame's node responses, in the terms the canvas
//! acts on.
//!
//! Every input pass on the canvas used to ask the same question its own
//! way — "which node's play chip was clicked", "which port was
//! double-clicked", "did anything on a node body get pressed" — by
//! polling [`Ui::response_for`] on one deterministic id per node, per
//! affordance, per pane. Six such sweeps ran every frame, and none of
//! them could be culled: the record has a viewport to skip against, but a
//! scan keyed by a document-unique id has nothing to skip against and
//! visited every node in the document however far off-screen it sat.
//!
//! This is those sweeps merged into one. It answers in domain coordinates
//! — a [`NodeId`] and which chip, a [`PortRef`] — so no consumer builds a
//! [`WidgetId`] any more, and the widget-id spelling of each affordance is
//! stated once here instead of in six places.
//!
//! **One writer.** `CanvasGeometry::rebuild` fills the whole digest as it
//! walks the scene in the prepass — [`CanvasHits::clear`] first, then
//! [`CanvasHits::note_node`] and [`CanvasHits::note_port`] per node. It has
//! to walk every node and every visible port anyway, and it needs the very
//! same responses: a node's body for its size and screen rect, a port's
//! circle for its center. Reading them there means an interaction costs no
//! poll of its own.
//!
//! **It culls itself.** A node's chips and ports are its descendants, so a
//! node whose *body* recorded nothing last frame had nothing under it to
//! interact with. That takes one response poll to establish — the same poll
//! the rebuild already wants — after which an off-screen node costs nothing
//! further. So the per-frame floor is one poll per node in the scene, and
//! everything past it is bounded by what is actually on screen.
//!
//! That floor is a consequence of palantir's response API being pull-by-id:
//! "was this node recorded" is only answerable by asking about its id, and a
//! [`WidgetId`] cannot be mapped back to the node it names. A palantir-side
//! accessor for the widget a press or click landed on would remove the walk
//! rather than shrink it.
//!
//! **The digest is read against a later scene than it was filled from.** It
//! holds ids from *last* frame's projection; a consumer confirms the node is
//! still in the pane it is drawing before acting, which is the same check it
//! needed anyway for a hit belonging to another pane.

use palantir::{ResponseState, Ui, WidgetId};
use scenarium::{InputPort, NodeId};

use crate::core::document::{PortKind, PortRef};
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph::node::drag_handles;
use crate::gui::pane::graph::node::header::{cache_eviction_badge_wid, play_badge_wid};
use crate::gui::pane::graph::node::port_row::{const_editor_wid, input_cell_wid};
use crate::gui::pane::graph::node::preview_row::preview_image_wid;
use crate::gui::pane::graph::paint::inspector::inspect_badge_wid;

/// A left-clickable chip on a node, named by what it does rather than by
/// the widget it lives in. One enum instead of a slot per chip: they are
/// found by the same loop, and a click reaches one widget, so at most one
/// can be hit per frame.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Chip {
    /// Header play chip — run this node's cone.
    Play,
    /// `↻` — drop this node's runtime caches.
    EvictCache,
    /// `i` — cycle the node's inspection panel.
    Inspect,
    /// A preview card's image area — open it in an image viewer.
    PreviewImage,
}

/// The drag handle a fresh body drag latched onto, and the node it moves.
#[derive(Copy, Clone, Debug)]
struct HandleLatch {
    node: NodeId,
    handle: WidgetId,
}

#[derive(Copy, Clone, Debug)]
struct ChipHit {
    node: NodeId,
    chip: Chip,
}

/// Last frame's canvas interactions. Every field is rewritten each frame by
/// [`Self::clear`] + [`Self::note_node`] / [`Self::note_port`]; the struct is
/// retained on `GraphUI` only so it has one owner, not to carry state between
/// frames.
///
/// Each slot is an `Option` rather than a collection because a pointer
/// button reaches one widget at a time — two nodes cannot both have had
/// their play chip clicked in one frame.
#[derive(Default, Debug)]
pub(crate) struct CanvasHits {
    chip: Option<ChipHit>,
    /// The node body's own left edge — a click, or a drag latching on it.
    /// The title is deliberately *not* folded in: a title drag moves the
    /// node but has never counted as "acted on a node body".
    body_acted: Option<NodeId>,
    latched: Option<HandleLatch>,
    menu: Option<NodeId>,
    port_dbl: Option<PortRef>,
    const_editor: Option<InputPort>,
}

impl CanvasHits {
    /// The node whose `chip` took this frame's click.
    pub(crate) fn chip(&self, chip: Chip) -> Option<NodeId> {
        self.chip.filter(|hit| hit.chip == chip).map(|hit| hit.node)
    }

    /// The node whose body was clicked or started a drag — "the user
    /// acted on a node", as opposed to on the bare canvas.
    pub(crate) fn body_acted(&self) -> Option<NodeId> {
        self.body_acted
    }

    /// The handle a body/title drag latched onto this frame, when it
    /// latched on `node`. A [`WidgetId`], not a domain id: the drag anchor
    /// polls that exact widget for its delta on later frames, which is the
    /// one thing here a consumer still needs the widget for.
    pub(crate) fn latched_on(&self, node: NodeId) -> Option<WidgetId> {
        self.latched
            .filter(|latch| latch.node == node)
            .map(|latch| latch.handle)
    }

    /// The node whose body was right-clicked, opening its context menu.
    pub(crate) fn menu(&self) -> Option<NodeId> {
        self.menu
    }

    /// The port whose circle — or, on an input, its label cell — was
    /// double-clicked.
    pub(super) fn double_clicked_port(&self) -> Option<PortRef> {
        self.port_dbl
    }

    /// The input whose inline const editor was clicked. Only an editor
    /// that draws a button acts on it (the `FsPath` picker); the caller
    /// decides that from the port's type.
    pub(super) fn clicked_const_editor(&self) -> Option<InputPort> {
        self.const_editor
    }

    /// Empty the digest, ahead of the walk that refills it whole.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Fold one node's interactions in, given the body response
    /// [`CanvasGeometry::rebuild`](super::geometry::CanvasGeometry::rebuild)
    /// already read for it — so the body is polled once for its size, its
    /// screen rect and its interactions together, rather than once by each.
    ///
    /// That response is also the cull test: `layout_rect` is `None` exactly
    /// when the node recorded nothing last frame, which is when none of its
    /// descendants can have been interacted with either.
    pub(super) fn note_node(&mut self, ui: &Ui, node: NodeCtx<'_>, body: ResponseState) {
        if body.left.clicked() || body.left.drag.started() {
            self.body_acted.get_or_insert(node.id);
        }
        // A node that didn't record has no chip or port that did — they
        // are its descendants — so the polls below would all return the
        // default. Skipping them is what keeps this pass proportional to
        // what is on screen rather than to the whole document.
        if body.layout_rect.is_none() {
            return;
        }
        if body.right.clicked() {
            self.menu.get_or_insert(node.id);
        }
        if let Some(handle) =
            drag_handles(node.id).find(|w| ui.response_for(*w).left.drag.started())
        {
            self.latched.get_or_insert(HandleLatch {
                node: node.id,
                handle,
            });
        }
        self.scan_chips(ui, node);
    }

    /// The header/body chips, each guarded by the same condition that
    /// decides whether it draws at all (`gui::pane::graph::node::header`,
    /// `gui::pane::graph::node::preview_row`) — so a stale response can't act on a node
    /// that has stopped offering the affordance, and that rule lives in
    /// one place per chip rather than in the chip's draw and its scan.
    fn scan_chips(&mut self, ui: &Ui, node: NodeCtx<'_>) {
        let candidates = [
            (Chip::Play, node.runnable(), play_badge_wid(node.id)),
            (
                Chip::EvictCache,
                node.can_evict_cache(),
                cache_eviction_badge_wid(node.id),
            ),
            (Chip::Inspect, true, inspect_badge_wid(node.id)),
            (
                Chip::PreviewImage,
                node.preview(),
                preview_image_wid(node.id),
            ),
        ];
        for (chip, drawn, wid) in candidates {
            if !drawn {
                continue;
            }
            let response = ui.response_for(wid);
            if response.left.clicked() {
                self.chip.get_or_insert(ChipHit {
                    node: node.id,
                    chip,
                });
            }
        }
    }

    /// Fold one port's interactions in, given the response
    /// `CanvasGeometry::rebuild` already read for its circle — so the
    /// circle is polled once for its center and its double-click together,
    /// rather than once by each. The input column's two extra widgets are
    /// polled here, where the rest of this file's widget-id knowledge
    /// lives; nothing else reads them.
    ///
    /// The port half of the digest fills from that pass rather than from
    /// [`Self::scan`] because the ports have to be walked there anyway.
    pub(super) fn note_port(&mut self, ui: &Ui, port: PortRef, circle: ResponseState) {
        if circle.left.double_clicked() {
            self.port_dbl.get_or_insert(port);
        }
        if port.kind != PortKind::Input {
            return;
        }
        // The circle intercepts its own rect; the cell catches the label.
        if ui.response_for(input_cell_wid(port)).left.double_clicked() {
            self.port_dbl.get_or_insert(port);
        }
        let input = InputPort::new(port.node_id, port.port_idx);
        if ui.response_for(const_editor_wid(input)).left.clicked() {
            self.const_editor.get_or_insert(input);
        }
    }
}
