//! [`NewNodeUi`]: the right-click / double-click palette that adds a node
//! where the click landed. The roster it lists is
//! [`NodePalette`].

pub(crate) mod node_palette;

use glam::Vec2;
use palantir::{Ui, WidgetId};

use crate::core::document::PortRef;
use crate::gui::pane::graph::canvas::outer_canvas_widget_id;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::gesture::canvas_gesture::CanvasGesture;
use crate::gui::pane::graph::gesture::new_node::node_palette::NodePalette;
use crate::gui::pane::graph::paint::anchored_menu::AnchoredMenu;
use crate::gui::requests::Requests;

/// Right-click or double-click on empty canvas → popup that lists every
/// `Func` the context's library holds plus the built-in specials, grouped by
/// category. Clicking an entry emits the intent that adds it at the click's
/// world position (inner-canvas pre-transform). Outside-click and Esc
/// dismiss.
#[derive(Default, Debug)]
pub(crate) struct NewNodeUi {
    menu: AnchoredMenu,
    /// Inner-canvas pre-transform position of the current open — the
    /// spawned node lands exactly under the click. Set at open, read at pick.
    world_pos: Vec2,
    /// Source port when a connection dropped on empty canvas opened the
    /// palette; on pick the wire resumes floating from it (rather than
    /// auto-attaching). `None` for a plain RMB / double-click. Set at open,
    /// read at pick.
    source: Option<PortRef>,
    /// Set when a node was picked from a popup that a dropped connection
    /// opened: the wire's source port, handed back to `ConnectionUI` so the
    /// wire resumes *floating* and the user clicks the exact port to land
    /// it. Taken by the canvas next frame.
    resume_floating: Option<PortRef>,
    /// The palette's search box. Cleared on each open; case-insensitively
    /// filters the listed entries by name (a matching category name shows
    /// that whole column). Empty ⇒ everything shows.
    search: Search,
}

/// The palette's search text and its case-folded copy.
///
/// Together, because the fold is derived from the text and every frame
/// needs it: keeping the buffer beside its source lets the fold reuse one
/// allocation instead of building a fresh `String` per frame.
#[derive(Default, Debug)]
struct Search {
    text: String,
    folded: String,
}

impl Search {
    fn fold(&mut self) {
        self.folded.clear();
        self.folded
            .extend(self.text.chars().flat_map(char::to_lowercase));
    }
}

impl NewNodeUi {
    /// Close the palette and drop the pending wire handoffs. The search
    /// buffers are cleared in place, keeping the allocation the fold reuses.
    pub(crate) fn reset(&mut self) {
        self.menu.reset();
        self.source = None;
        self.resume_floating = None;
        self.search.text.clear();
        self.search.folded.clear();
    }

    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        cx: CanvasCtx<'_>,
        pending_source: Option<PortRef>,
        out: &mut Requests,
    ) {
        let graph_ctx = cx.graph_ctx();
        let resp = ui.response_for(outer_canvas_widget_id());
        // Open the palette either from a bare RMB / double-click (`NewNode`
        // gesture) or from a connection dropped on empty canvas
        // (`pending_source`). Placement is the same — under the pointer.
        let mut just_opened = false;
        if (pending_source.is_some() || cx.gesture() == Some(CanvasGesture::NewNode))
            && let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect)
        {
            self.world_pos = graph_ctx.viewport().to_world(local);
            self.source = pending_source;
            self.menu.open_at(rect.min + local);
            // Fresh open: empty the filter, read the graph's own
            // definitions once, and focus the search field this frame so
            // the user can type straight away.
            self.search.text.clear();
            just_opened = true;
        }

        // Everything below is per-open work — the height arithmetic reads
        // the display and a last-frame rect — so nothing but an open
        // palette on *this* pane should pay for it. `show` would answer
        // `None` anyway, but only after its arguments were built.
        if !self.menu.open_on() {
            return;
        }

        // Cap the palette height to the window so a short window scrolls
        // the overflow (via the inner vertical `Scroll`) instead of
        // running off-screen. The popup's `max_size` height bounds the
        // whole popup; the search row sits above a `Scroll` whose own cap
        // (`max_height` minus the chrome above it) keeps it from eating the
        // header's space — a `Hug` scroll otherwise claims the full cap.
        let surface = ui.display().logical_rect();
        let max_height = graph_ctx
            .theme()
            .new_node_popup_max_height
            .min(surface.size.h - 16.0)
            .max(120.0);
        let scroll_cap = (max_height - chrome_above_results(ui)).max(MIN_RESULTS_HEIGHT);
        let search = &mut self.search;
        // Rows are picked at the position the *open* captured, not wherever
        // the pointer has drifted to by the frame of the click.
        let pos = self.world_pos;
        // Inside the body, so the per-frame read only happens on the frames
        // the palette is actually up — `show` skips the body when closed.
        let chosen = self
            .menu
            .show(ui, "new_node_popup", Some(max_height), |ui, popup| {
                let palette = NodePalette::new(graph_ctx, pos);
                palette.body(ui, popup, search, scroll_cap, just_opened)
            });

        if let Some(intent) = chosen {
            out.push_graph(intent);
            // If a dropped connection opened this popup, hand its source
            // back so the wire resumes floating — the user then clicks the
            // exact port to land it, rather than it auto-attaching.
            self.resume_floating = self.source;
        }
    }

    /// Take the source of a wire whose drop spawned a node this frame — the
    /// canvas re-floats it on `ConnectionUI`. `None` on a plain palette open.
    pub(crate) fn take_resume_floating(&mut self) -> Option<PortRef> {
        self.resume_floating.take()
    }
}

/// Gap (px) below the search field, before the results scroll.
const SEARCH_ROW_GAP: f32 = 8.0;

/// Floor under the results area, so a window short enough that the chrome eats
/// the whole cap still shows a scrollable strip of rows rather than nothing.
const MIN_RESULTS_HEIGHT: f32 = 80.0;

/// Stable id for the palette's search field. A free function rather than a
/// `const` because the height cap below reads the field's rect *before* the
/// body that records it runs, so both sites have to name the same id.
pub(crate) fn search_field_wid() -> WidgetId {
    WidgetId::from_hash("new_node_search")
}

/// Stable id for the scrolling results area, the half [`chrome_above_results`]
/// sizes. Explicit like the search field's so the two ends of that arithmetic
/// are both addressable.
pub(crate) fn results_wid() -> WidgetId {
    WidgetId::from_hash("new_node_results")
}

/// Vertical space the palette's chrome claims above the scrolling results: the
/// popup's own padding, the gutter between its two children, the search field,
/// and [`SEARCH_ROW_GAP`] under it.
///
/// The inner `Scroll` needs this subtracted from the popup's height cap
/// because a stack hands every non-`Fill` child its *full* main extent — a
/// `Hug` scroll offered the whole cap takes it, and the search row above then
/// pushes the popup past the cap. `Sizing::FILL` is not the way out: palantir
/// clears a scroll's fit flag on any axis the caller didn't `Hug`, so a filled
/// scroll reports zero desired height and the popup collapses onto its search
/// row.
///
/// Every term is read rather than assumed — the field's height off its own
/// last-frame rect, the rest off the theme slot the popup is built from — so
/// restyling the field's text or the menu's padding resizes the results area
/// with it instead of silently mis-sizing the scroll. Only the first frame of
/// the first open has no rect yet and falls back to one line of body text.
fn chrome_above_results(ui: &Ui) -> f32 {
    let menu = &ui.theme().context_menu;
    // An arranged rect is margin-inclusive, so the field's already carries
    // [`SEARCH_ROW_GAP`]; only the bare-line fallback has to add it.
    let row = ui.response_for(search_field_wid()).layout_rect.map_or_else(
        || {
            let text = &ui.theme().text;
            text.line_height_for(text.font_size_px) + SEARCH_ROW_GAP
        },
        |rect| rect.size.h,
    );
    menu.padding.vert() + menu.gap + row
}

#[cfg(test)]
mod tests;
