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
//! **Two writers, one per phase, and they do not overlap.**
//! [`CanvasHits::scan`] clears the digest and fills the node half in the
//! navigation phase. `CanvasGeometry::rebuild` fills the port half later,
//! in the prepass, because it has to walk every visible port anyway — so a
//! port's double-click rides the same response read as its center instead
//! of costing a poll of its own.
//!
//! **It culls itself.** A node's chips are its descendants, so a node
//! whose *body* recorded nothing last frame had nothing under it to
//! interact with. That takes one response poll to establish — the same
//! poll whose click and drag edges the sweep already wants — after which
//! an off-screen node costs nothing further. So the per-frame floor is
//! one poll per node in the scene, and everything past it is bounded by
//! what is actually on screen. `rebuild` culls the port half on the same
//! test, for the same reason.
//!
//! **The node half is read against a later scene than it was filled
//! from.** [`CanvasHits::scan`] runs before `Editor::rebuild_scene`,
//! because two of its consumers (the graph-open and preview-open chips)
//! have to resolve before the tab set settles. Its hits are therefore ids
//! from *last* frame's projection; a consumer confirms the node is still
//! in the pane it is drawing (`Pane::contains`) before acting, which
//! is the same check it needed anyway for a hit belonging to another pane.
//! The port half has no such gap — it fills after the rebuild.

use palantir::{ResponseState, Ui, WidgetId};
use scenarium::{InputPort, NodeId};

use crate::core::document::{PortKind, PortRef};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::node_scope::NodeScope;
use crate::gui::pane::graph::node::header::{cache_eviction_badge_wid, play_badge_wid};
use crate::gui::pane::graph::node::port_row::{const_editor_wid, input_cell_wid};
use crate::gui::pane::graph::node::preview_row::preview_image_wid;
use crate::gui::pane::graph::node::{drag_handles, node_widget_id};
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

/// Last frame's canvas interactions. Every field is rewritten by
/// [`Self::scan`]; the struct is retained on `GraphUI` only so it has one
/// owner, not to carry state between frames.
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

    /// Clear the digest and refill its node half from last frame's
    /// responses, across every visible pane. Run once per frame, in the
    /// navigation phase — see the module docs for why there, what it
    /// costs, and which pass fills the port half.
    pub(crate) fn scan(&mut self, ui: &Ui, graph: GraphCtx<'_>) {
        *self = Self::default();
        if !graph.is_visible() {
            return;
        }
        for node in graph.nodes() {
            self.scan_node(ui, node);
        }
    }

    fn scan_node(&mut self, ui: &Ui, node: NodeScope<'_>) {
        let body = ui.response_for(node_widget_id(node.id));
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
    /// decides whether it draws at all (`gui::node::header`,
    /// `gui::node::preview_row`) — so a stale response can't act on a node
    /// that has stopped offering the affordance, and that rule lives in
    /// one place per chip rather than in the chip's draw and its scan.
    fn scan_chips(&mut self, ui: &Ui, node: NodeScope<'_>) {
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
