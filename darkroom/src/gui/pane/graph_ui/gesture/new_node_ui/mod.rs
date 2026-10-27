//! [`NewNodeUi`]: the right-click / double-click palette that adds a node
//! where the click landed. The roster it lists is
//! [`NodePalette`].

pub(crate) mod node_palette;
pub(crate) mod palette_rows;

use glam::Vec2;
use palantir::widget::domain;
use palantir::{Ui, WidgetId};

use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::gesture::anchored_menu::{AnchoredMenu, PopupShape};
use crate::gui::pane::graph_ui::gesture::new_node_ui::node_palette::{NodePalette, Opening};
use crate::gui::pane::graph_ui::gesture::new_node_ui::palette_rows::PaletteRows;
use crate::gui::pane::graph_ui::prepass_ctx::CanvasGesture;
use crate::gui::port_ref::PortRef;

/// Right-click or double-click on empty canvas → popup that lists every
/// `Func` the context's library holds plus the built-in specials: under their
/// categories with no query, ranked by how they match with one. A click or
/// Enter on the focused row emits the intent that adds it at the click's world
/// position (inner-canvas pre-transform). Outside-click and Esc dismiss.
#[derive(Default, Debug)]
pub(crate) struct NewNodeUi {
    menu: AnchoredMenu,
    /// Inner-canvas pre-transform position of the current open — the
    /// spawned node lands exactly under the click. Set at open, read at pick.
    world_pos: Vec2,
    /// Source port when a wire dropped on empty canvas opened the
    /// palette; on pick the wire resumes floating from it (rather than
    /// auto-attaching). `None` for a plain RMB / double-click. Set at open,
    /// read at pick.
    source: Option<PortRef>,
    /// The palette's search box. Cleared on each open; case-insensitively
    /// filters and ranks the listed entries by name, category and
    /// description. Empty ⇒ everything shows.
    search: Search,
}

/// The palette's search text, its case-folded copy, and the rows it matched.
///
/// Together, because the fold is derived from the text and every frame
/// needs it: keeping the buffer beside its source lets the fold reuse one
/// allocation instead of building a fresh `String` per frame. The rows are
/// filtered again only when the fold changes.
#[derive(Default, Debug)]
struct Search {
    text: String,
    folded: String,
    rows: PaletteRows,
    /// The row Enter picks, by its index in the list; the first after each filtering.
    focused: usize,
}

impl Search {
    /// Fold this frame's text, and report whether the fold changed — the
    /// palette's cue to filter its rows again. Compared before it is
    /// rebuilt, so an unchanged query costs one pass and no write.
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
    /// Close the palette and drop the pending wire handoffs. The search
    /// buffers are cleared in place, keeping the allocation the fold reuses.
    pub(crate) fn reset(&mut self) {
        self.menu.reset();
        self.source = None;
        self.search.text.clear();
        self.search.folded.clear();
    }

    /// Whether the palette is open.
    pub(crate) const fn in_flight(&self) -> bool {
        self.menu.is_open()
    }

    /// Open the palette from a bare RMB / double-click, or from a wire dropped on empty canvas
    /// this frame, and record it while it is open, the `AddNode` a pick raises onto `out`.
    /// Returns the source of the dropped wire that opened it, when a pick lands, which floats
    /// again so the user clicks the exact port to land it rather than it auto-attaching.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        ctx: CanvasCtx<'_>,
        out: &mut Requests,
    ) -> Option<PortRef> {
        let pending_source = ctx.dropped_wire();
        let graph_ctx = ctx.graph_ctx();
        let resp = ui.response_for(canvas::outer_wid());
        // Open the palette either from a bare RMB / double-click (`NewNode`
        // gesture) or from a wire dropped on empty canvas
        // (`pending_source`). Placement is the same — under the pointer.
        let opening = if (pending_source.is_some() || ctx.gesture() == Some(CanvasGesture::NewNode))
            && let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect)
        {
            self.world_pos = graph_ctx.viewport().to_world(local);
            self.source = pending_source;
            self.menu.open_at(rect.min + local);
            // Fresh open: empty the filter, read the graph's own
            // definitions once, and focus the search field this frame so
            // the user can type straight away.
            self.search.text.clear();
            Opening::Fresh
        } else {
            Opening::Standing
        };

        // Everything below is per-open work — the height arithmetic reads
        // the display and a last-frame rect — so nothing but an open palette
        // should pay for it. `show` would answer `None` anyway, but only
        // after its arguments were built.
        if !self.menu.is_open() {
            return None;
        }

        // Cap the palette height to the window so a short window scrolls
        // the overflow (via the inner vertical `Scroll`) instead of
        // running off-screen. The popup's `max_size` height bounds the
        // whole popup; the search row sits above a `Scroll` whose own cap
        // (`max_height` minus the space above it) keeps it from eating the
        // header's space — a `Hug` scroll otherwise claims the full cap.
        let surface = ui.display().logical_rect();
        let max_height = popup_cap(graph_ctx.theme().new_node_popup_max_height, surface.size.h);
        let scroll_cap = (max_height - space_above_results(ui)).max(MIN_RESULTS_HEIGHT);
        let search = &mut self.search;
        // Rows are picked at the position the *open* captured, not wherever
        // the pointer has drifted to by the frame of the click.
        let pos = self.world_pos;
        // Inside the body, so the per-frame read only happens on the frames
        // the palette is actually up — `show` skips the body when closed.
        let shape = PopupShape {
            id_salt: "new_node_popup",
            max_height: Some(max_height),
        };
        let chosen = self.menu.show(ui, shape, |ui, popup| {
            let palette = NodePalette {
                graph_ctx,
                pos,
                scroll_cap,
                opening,
            };
            palette.body(ui, popup, search)
        });

        let picked = chosen?;
        out.push_graph(picked);
        self.source
    }
}

/// The palette's height cap: the theme's, held inside a window `surface_h`
/// tall by [`POPUP_WINDOW_MARGIN`], and never below [`POPUP_MIN_HEIGHT`].
fn popup_cap(theme_max: f32, surface_h: f32) -> f32 {
    theme_max
        .min(surface_h - POPUP_WINDOW_MARGIN)
        .max(POPUP_MIN_HEIGHT)
}

/// What the palette leaves of a short window's height, so it never touches
/// the window's edge.
const POPUP_WINDOW_MARGIN: f32 = 16.0;

/// Floor under the palette's height: a window too short for the theme's cap
/// still gets a palette with a search field and a few rows.
const POPUP_MIN_HEIGHT: f32 = 120.0;

/// Gap (px) below the search field, before the results scroll.
const SEARCH_ROW_GAP: f32 = 8.0;

/// Floor under the results area, so a window short enough that the space above the results eats
/// the whole cap still shows a scrollable strip of rows rather than nothing.
const MIN_RESULTS_HEIGHT: f32 = 80.0;

/// Stable id for the palette's search field. A free function rather than a
/// `const` because the height cap below reads the field's rect *before* the
/// body that records it runs, so both sites have to name the same id.
pub(crate) fn search_field_wid() -> WidgetId {
    WidgetId::from_hash("new_node_search")
}

/// Stable id for the row the search field sits in, whose top edge [`space_above_results`]
/// measures from: the row pads the field, so the field's own top is below it.
pub(crate) fn search_row_wid() -> WidgetId {
    WidgetId::from_hash("new_node_search_row")
}

/// Stable id for the scrolling results area, the half [`space_above_results`]
/// sizes. Explicit like the search field's so the two ends of that arithmetic
/// are both addressable.
pub(crate) fn results_wid() -> WidgetId {
    WidgetId::from_hash("new_node_results")
}

/// Vertical space the palette claims above the scrolling results: the
/// popup's own padding and border, plus everything the layout placed between
/// the search row's top edge and the results' top edge.
///
/// The inner `Scroll` needs this subtracted from the popup's height cap
/// because a stack hands every non-`Fill` child its *full* main extent — a
/// `Hug` scroll offered the whole cap takes it, and the search row above then
/// pushes the popup past the cap. `Sizing::FILL` is not the way out: palantir
/// clears a scroll's fit flag on any axis the caller didn't `Hug`, so a filled
/// scroll reports zero desired height and the popup collapses onto its search
/// row.
///
/// Every term is read rather than assumed — the two rows off their own
/// last-frame rects, the padding and border off the theme slot the popup is
/// built from — so restyling the field's text or the menu's look resizes the
/// results area with it instead of silently mis-sizing the scroll.
///
/// Palantir folds a chrome's border into its padding on every side, unless the
/// width paints nothing. That fold (`Chrome::border_inset`) is not public,
/// so its gate is repeated here over the same public `domain` predicate.
///
/// The span between the rows is measured rather than re-summed from the
/// field's height, [`SEARCH_ROW_GAP`] and the stack's gutter. A sum has to be
/// kept in step with what `body` records, and a term missing from it shrinks
/// the space by that much, which lands as rows painted past the popup's
/// bottom edge. Measuring covers whatever the layout put between the rows,
/// including a row added later. Only the first frame of the first open has no
/// rects yet, and estimates the span from one line of body text.
fn space_above_results(ui: &Ui) -> f32 {
    let menu = &ui.theme().context_menu;
    let field = ui.response_for(search_row_wid()).layout_rect;
    let results = ui.response_for(results_wid()).layout_rect;
    let rows = field.zip(results).map_or_else(
        || {
            let text = &ui.theme().text;
            text.line_height_for(text.font_size) + SEARCH_ROW_GAP + menu.gap
        },
        |(field, results)| results.min.y - field.min.y,
    );
    let border = menu.panel.border.width;
    let border = if domain::is_invisible(border) {
        0.0
    } else {
        border
    };
    menu.padding.sums().h + 2.0 * border + rows
}

#[cfg(test)]
mod tests;
