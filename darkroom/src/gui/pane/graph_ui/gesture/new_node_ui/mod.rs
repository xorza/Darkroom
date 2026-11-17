//! [`NewNodeUi`]: the right-click / double-click palette that adds a node
//! where the click landed. The roster it lists is
//! [`NodePalette`].

mod landing;
pub(crate) mod node_palette;
pub(crate) mod palette_rows;

use std::mem;

use glam::Vec2;
use palantir::widget::domain;
use palantir::{Ui, WidgetId};
use scenarium::NodeId;

use crate::core::edit::compound_intent::CompoundIntent;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::canvas_input::CanvasGesture;
use crate::gui::pane::graph_ui::gesture::anchored_menu::{AnchoredMenu, PopupShape};
use crate::gui::pane::graph_ui::gesture::new_node_ui::landing::DroppedEnd;
use crate::gui::pane::graph_ui::gesture::new_node_ui::node_palette::{NodePalette, Opening};
use crate::gui::pane::graph_ui::gesture::new_node_ui::palette_rows::PaletteRows;
use crate::gui::port_ref::PortRef;
use crate::gui::theme::Theme;

/// Right-click or double-click on empty canvas → popup that lists every
/// `Func` the context's library holds plus the built-in specials: under their
/// categories with no query, ranked by how they match with one. A click or
/// Enter on the focused row emits the intent that adds it at the click's world
/// position (inner-canvas pre-transform). Outside-click and Esc dismiss.
#[derive(Default, Debug)]
pub(crate) struct NewNodeUi {
    menu: AnchoredMenu,
    /// Inner-canvas pre-transform position of the current open, so the spawned node lands
    /// exactly under the click.
    world_pos: Vec2,
    /// Source port when a wire dropped on empty canvas opened the palette, which the rows land
    /// on a port of their new node. `None` for a plain RMB or double-click.
    source: Option<PortRef>,
    /// The palette's search box. Cleared on each open; case-insensitively filters and ranks the
    /// listed entries by name, category and description. Empty shows everything.
    search: Search,
    /// Whether the input stage opened the palette for the record to come.
    opening: Opening,
    /// The node a pick replaces, when Replace with… opened the palette: a pick then raises the
    /// Replace dialog instead of adding a node.
    replacing: Option<NodeId>,
}

/// The palette's search text, its case-folded copy, and the rows it matched.
///
/// Together, so the fold reuses one allocation instead of building a fresh `String` per frame.
/// The rows are filtered again only when the fold changes.
#[derive(Default, Debug)]
struct Search {
    text: String,
    folded: String,
    rows: PaletteRows,
    /// The row Enter picks, by its index in the list; the first after each filtering.
    focused: usize,
}

impl Search {
    /// Fold this frame's text, and report whether the fold changed, the palette's cue to filter
    /// its rows again. Compared before it is rebuilt, so an unchanged query costs no write.
    fn fold(&mut self) -> bool {
        let lowered = || self.text.chars().flat_map(char::to_lowercase);
        if lowered().eq(self.folded.chars()) {
            return false;
        }
        self.folded.clear();
        self.folded.extend(lowered());
        true
    }
}

impl NewNodeUi {
    /// Close the palette and drop the pending wire handoffs, keeping the search buffers'
    /// allocation.
    pub(crate) fn reset(&mut self) {
        self.menu.reset();
        self.source = None;
        self.search.text.clear();
        self.search.folded.clear();
        self.opening = Opening::Standing;
        self.replacing = None;
    }

    /// Whether the palette is open.
    pub(crate) const fn in_flight(&self) -> bool {
        self.menu.is_open()
    }

    /// Open the palette from a bare RMB / double-click, or from a wire `dropped` on empty canvas
    /// this frame. Placement is the same — under the pointer.
    pub(crate) fn open(&mut self, ui: &Ui, ctx: CanvasCtx<'_>, dropped: Option<PortRef>) {
        if dropped.is_none() && ctx.gesture() != Some(CanvasGesture::NewNode) {
            return;
        }
        let resp = ui.response_for(canvas::outer_wid(ctx.graph_ctx().graph()));
        let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect) else {
            return;
        };
        self.world_pos = ctx.graph_ctx().camera().to_world(local);
        self.source = dropped;
        self.replacing = None;
        self.menu.open_at(rect.min + local);
        // Fresh open: empty the filter, and have the record filter afresh and focus the search
        // field so the user can type straight away.
        self.search.text.clear();
        self.opening = Opening::Fresh;
    }

    /// Open the palette as Add Node asks: under the pointer, or at the pane's center when the
    /// pointer is outside it, as a pick from the menu bar leaves it.
    pub(crate) fn open_at_pointer(&mut self, ui: &Ui, graph_ctx: GraphCtx<'_>) {
        let resp = ui.response_for(canvas::outer_wid(graph_ctx.graph()));
        let Some(rect) = resp.rect else {
            return;
        };
        let size = Vec2::new(rect.size.w, rect.size.h);
        let inside = |local: &Vec2| {
            local.x >= 0.0 && local.y >= 0.0 && local.x <= size.x && local.y <= size.y
        };
        let local = resp.pointer_local.filter(inside).unwrap_or(size * 0.5);
        self.world_pos = graph_ctx.camera().to_world(local);
        self.source = None;
        self.replacing = None;
        self.menu.open_at(rect.min + local);
        self.search.text.clear();
        self.opening = Opening::Fresh;
    }

    /// Open the palette under the pointer to pick the node that replaces `node_id`, as Replace
    /// with… asks.
    pub(crate) fn open_replacing(&mut self, ui: &mut Ui, node_id: NodeId) {
        let Some(pointer) = ui.pointer_pos() else {
            return;
        };
        self.source = None;
        self.replacing = Some(node_id);
        self.menu.open_at(pointer);
        self.search.text.clear();
        self.opening = Opening::Fresh;
    }

    /// Record the palette while it is open, and raise what a pick means: the `AddNode`, with the
    /// wire whose drop opened it laid onto the port its row names, or the Replace dialog when it
    /// was opened to replace a node.
    pub(crate) fn record(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let graph_ctx = stage.ctx;
        // Everything below is per-open work (the height arithmetic reads the display and a
        // last-frame rect), which `show` would not skip, as its arguments are built first.
        if !self.menu.is_open() {
            return;
        }
        let opening = mem::take(&mut self.opening);

        // Cap the palette height to the window so a short window scrolls the overflow instead of
        // running off-screen. The popup's `max_size` height bounds the whole popup; the inner
        // `Scroll`'s own cap (`max_height` minus the space above it) keeps it from eating the
        // search row's space, as a `Hug` scroll otherwise claims the full cap.
        let surface = ui.display().logical_rect();
        let max_height = popup_cap(graph_ctx.theme(), surface.size.h);
        let scroll_cap =
            (max_height - space_above_results(ui, graph_ctx.theme())).max(MIN_RESULTS_HEIGHT);
        let search = &mut self.search;
        // Rows are picked at the position the open captured, not where the pointer has drifted.
        let pos = self.world_pos;
        let shape = PopupShape {
            id_salt: "new_node_popup",
            max_height: Some(max_height),
        };
        let replacing = self.replacing;
        let dropped = self
            .source
            .and_then(|source| DroppedEnd::of(graph_ctx, source));
        let chosen = self.menu.show(ui, shape, |ui| {
            let palette = NodePalette {
                graph_ctx,
                pos,
                scroll_cap,
                opening,
                replacing: replacing.is_some(),
                dropped,
            };
            palette.body(ui, search)
        });

        let Some(picked) = chosen else {
            return;
        };
        if let Some(node_id) = self.replacing.take() {
            let GraphIntent::AddNode { node, .. } = picked.add else {
                unreachable!("a palette pick adds a node");
            };
            debug_assert!(
                picked.compounds.is_empty(),
                "a replace lists no library entry"
            );
            stage.out.push_app(AppCommand::ProposeReplace {
                graph: stage.ctx.graph(),
                node_id,
                with: Box::new(node),
            });
            return;
        }
        for compound in picked.compounds {
            stage
                .out
                .push_compound(CompoundIntent::Add(Box::new(compound)));
        }
        // One batch, so the node and its wire are one history entry.
        let graph = stage.ctx.graph();
        stage.out.push_graph(graph, picked.add);
        stage.out.extend_graph(graph, picked.wire);
    }
}

/// The palette's height cap: [`POPUP_MAX_HEIGHT`], held inside a window `surface_h` tall with
/// the theme's `space.lg` of it left over, a popup's distance from the window edge, and never
/// below [`POPUP_MIN_HEIGHT`]. The popup's placement fits it into what is left.
fn popup_cap(theme: &Theme, surface_h: f32) -> f32 {
    POPUP_MAX_HEIGHT
        .min(surface_h - theme.space.lg)
        .max(POPUP_MIN_HEIGHT)
}

/// The palette's height when the window has room: past it, the results scroll.
const POPUP_MAX_HEIGHT: f32 = 400.0;

/// Floor under the palette's height: a window too short for the theme's cap still gets a search
/// field and a few rows.
const POPUP_MIN_HEIGHT: f32 = 120.0;

/// Floor under the results area, so a window short enough that the space above the results eats
/// the whole cap still shows a scrollable strip of rows rather than nothing.
const MIN_RESULTS_HEIGHT: f32 = 80.0;

/// Stable id for the palette's search field, which the height cap reads before the body that
/// records it runs, so both sites name the same id.
pub(crate) fn search_field_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.palette.search")
}

/// Stable id for the row the search field sits in, whose top edge [`space_above_results`]
/// measures from: the row pads the field, so the field's own top is below it.
pub(crate) fn search_row_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.palette.search_row")
}

/// Stable id for the scrolling results area, which [`space_above_results`] sizes.
pub(crate) fn results_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.palette.results")
}

/// Vertical space the palette claims above the scrolling results: the
/// popup's own padding and border, plus everything the layout placed between
/// the search row's top edge and the results' top edge.
///
/// The inner `Scroll` needs this subtracted from the popup's height cap because a stack hands
/// every non-`Fill` child its full main extent: a `Hug` scroll offered the whole cap takes it,
/// and the search row above then pushes the popup past the cap. `Sizing::FILL` is no way out:
/// palantir clears a scroll's fit flag on any axis the caller didn't `Hug`, so a filled scroll
/// reports zero desired height and the popup collapses onto its search row.
///
/// Every term is read off the last-frame rects and the theme slot the popup is built from, so
/// restyling resizes the results area with it. Palantir folds a chrome's border into its
/// padding on every side, unless the width paints nothing; that fold (`Chrome::border_inset`)
/// is not public, so its gate is repeated here over the same public `domain` predicate.
///
/// The span between the rows is measured rather than re-summed from the field's height, the
/// gap under the search row and the stack's gutter: a sum must be kept in step with what `body`
/// records, and a missing term lands as rows painted past the popup's bottom edge. Only the
/// first frame of the first open has no rects yet, and estimates the span from one line of
/// body text.
fn space_above_results(ui: &Ui, theme: &Theme) -> f32 {
    let menu = &ui.theme().menu;
    let field = ui.response_for(search_row_wid()).layout_rect;
    let results = ui.response_for(results_wid()).layout_rect;
    let rows = field.zip(results).map_or_else(
        || {
            let text = &ui.theme().text;
            text.block_height(1) + theme.space.sm + menu.gap
        },
        |(field, results)| results.min.y - field.min.y,
    );
    let border = menu.panel.paint.border.width;
    let border = if domain::is_invisible(border) {
        0.0
    } else {
        border
    };
    menu.padding.sums().h + 2.0 * border + rows
}

#[cfg(test)]
mod tests;
