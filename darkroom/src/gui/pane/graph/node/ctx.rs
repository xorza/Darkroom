//! The leaf of the UI's context chain: one node of one graph pane.

use palantir::Ui;

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::node_scope::NodeScope;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::frame::hits::CanvasHits;
use crate::gui::pane::graph::node::node_hovered;
use crate::gui::pane::graph::paint::inspector::Inspectors;
use crate::gui::theme::{StaticValueEditorTheme, Theme};

/// One node of one graph pane, as its own subtree records it: the pane's
/// [`DrawCtx`] plus the node and whether the pointer is over it.
///
/// The leaf level of the context chain, and the one where the *item* is the
/// level — every function below a node body is about that node, so passing it
/// beside the context would be the same argument twice.
///
/// **Why the hover lives here.** Two things hang off it — the const editors'
/// hover-revealed chips ([`Self::sve`]) and whether the port rows build their
/// tooltips at all ([`Self::tips`]) — and both used to be resolved in
/// `ports_row` and threaded down as a `&StaticValueEditorTheme` and a `bool`
/// that no signature said were the same fact. Resolving it at the node body
/// instead costs one extra [`hover_within`](palantir::Ui::hover_within) for a
/// node with no ports (which returns before it would have asked); that read is
/// a hover-target comparison, not a rect test, so it is a lookup either way.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeCtx<'a> {
    draw: DrawCtx<'a>,
    node: NodeScope<'a>,
    hovered: bool,
}

impl<'a> NodeCtx<'a> {
    /// Narrow a pane's draw to one node — the context its own subtree records
    /// against. Resolves the pointer-over-node question here, once, for
    /// everything below that asks it.
    pub(crate) fn for_node<'n: 'a>(draw: DrawCtx<'a>, ui: &Ui, node: NodeScope<'n>) -> Self {
        Self {
            draw,
            node,
            hovered: node_hovered(ui, node.id),
        }
    }

    pub(crate) fn node(self) -> NodeScope<'a> {
        self.node
    }

    pub(crate) fn theme(self) -> &'a Theme {
        self.draw.theme()
    }

    pub(crate) fn graph_ctx(self) -> GraphCtx<'a> {
        self.draw.graph_ctx()
    }

    pub(crate) fn geometry(self) -> &'a CanvasGeometry {
        self.draw.geometry()
    }

    pub(crate) fn hits(self) -> &'a CanvasHits {
        self.draw.hits()
    }

    pub(crate) fn inspectors(self) -> &'a Inspectors {
        self.draw.inspectors()
    }

    /// Whether this node paints selected.
    pub(crate) fn is_selected(self) -> bool {
        self.draw.is_selected(self.node.id)
    }

    /// The pane-wide draw this node belongs to — for the readers that want
    /// something about the whole pane rather than this node, like the
    /// effective selection *set* a group drag snapshots.
    pub(crate) fn draw_ctx(self) -> DrawCtx<'a> {
        self.draw
    }

    /// Whether the port rows build their hover tooltips: their text is
    /// composed per port per frame, and no port can be showing one while the
    /// pointer is elsewhere, so only the node under it pays.
    pub(crate) fn tips(self) -> bool {
        self.hovered
    }

    /// The const-editor styling this node's value cells paint with. Pointer
    /// over the node surfaces the (otherwise invisible) chips at half
    /// strength — the edit affordance appears exactly when the pointer is in
    /// the neighborhood, and geometry never changes.
    pub(crate) fn sve(self) -> &'a StaticValueEditorTheme {
        let theme = self.theme();
        if self.hovered {
            &theme.static_value_editor_revealed
        } else {
            &theme.static_value_editor
        }
    }
}
