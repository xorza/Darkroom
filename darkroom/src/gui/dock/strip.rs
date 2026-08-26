//! A pane's tab strip. Renders one chip per open tab in a [`TabGroup`]
//! and highlights the active one (accent cap when the group is focused).
//! Draw-only: clicks and drags are read back by
//! [`DockUi::scan`](super::DockUi::scan) during prepass through the
//! deterministic widget ids exported here. The right-click split menu
//! is the one record-time emitter (its picks are this-frame values,
//! pushed as `GraphIntent`s directly). Pure view state; never touches the
//! document.

use palantir::{
    Align, Background, Configure, ContextMenu, Corners, InternedStr, MenuItem, Panel, Sense,
    Sizing, Spacing, Text, TextStyle, Ui, VAlign, WidgetId,
};

use crate::core::document::TabRef;
use crate::core::document::dock::TabGroupId;
use crate::core::document::dock::dock_op::{DockDrop, DockOp};
use crate::core::document::dock::split_side::SplitSide;
use crate::core::document::dock::tab_group::TabGroup;
use crate::gui::requests::Requests;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{colored_text, muted_text};

/// One chip's whole draw state, resolved from its group by
/// [`tab_labels`](super::tab_labels): the tab, its label text and unsaved
/// state (the projections that need the open document), and the two flags
/// that say which chip is its group's visible one and whether that group
/// holds the dock focus.
///
/// `active` lives here rather than being re-derived by position because
/// the strip walks *labels* while `TabGroup::active` indexes `tabs` — a
/// slice that didn't correspond 1:1 would silently cap the wrong chip.
/// Closability and renamability stay derived from `tab` itself.
#[derive(Debug)]
pub(super) struct TabLabel {
    pub(super) tab: TabRef,
    pub(super) text: InternedStr,
    /// This tab is its group's visible one.
    pub(super) active: bool,
    /// This tab's group holds the dock focus — the accent cap dims
    /// elsewhere so one pane always reads as "where actions go".
    pub(super) focused: bool,
    /// The open document has unsaved changes. Inks the dot on the chips
    /// that [`shows_document`] — the document-wide flag rides on every
    /// label because it is the *tab kind*, not the label, that decides
    /// which chips reserve the dot.
    pub(super) dirty: bool,
}

/// Every tab except the pinned `Main` graph carries a close button.
pub(super) fn closable(tab: TabRef) -> bool {
    tab != TabRef::Graph
}

/// Which tabs show the open document's own content, and so carry its
/// unsaved-changes dot. Preferences are app state and a viewer shows a run's
/// output — neither is in the file, so neither reserves the slot.
fn shows_document(tab: TabRef) -> bool {
    tab == TabRef::Graph
}

/// Stable id for `tab`'s chip — deterministic so the prepass
/// (activation clicks, the drag scan) can read it without the live
/// response. Keyed on the tab, not its strip slot: the prepass reads
/// *last* frame's response, and undo can have rearranged the strip since,
/// so a slot-keyed id would hand one chip's click to another tab.
pub(crate) fn tab_chip_wid(tab: TabRef) -> WidgetId {
    WidgetId::from_hash(("dock.tab", tab))
}

/// Stable id for `group`'s whole strip row — the drag scan's
/// insertion-zone rect.
pub(super) fn strip_wid(group: TabGroupId) -> WidgetId {
    WidgetId::from_hash(("dock.strip", group))
}

/// Stable id for `tab`'s close button.
pub(super) fn tab_close_wid(tab: TabRef) -> WidgetId {
    WidgetId::from_hash(("dock.tab_close", tab))
}

/// Stable id for `tab`'s unsaved-changes dot — the box, which a document tab
/// records whether or not the dot is inked.
pub(crate) fn tab_dirty_wid(tab: TabRef) -> WidgetId {
    WidgetId::from_hash(("dock.tab_dirty", tab))
}

/// Stable id for `tab`'s split context menu.
fn tab_menu_wid(tab: TabRef) -> WidgetId {
    WidgetId::from_hash(("dock.tab_menu", tab))
}

/// Diameter of the unsaved-changes dot.
const DIRTY_DOT: f32 = 3.5;

/// The unsaved-changes dot: a filled circle when the document differs from
/// its file, an empty box of the same size when it does not.
///
/// Always recorded — never skipped on a saved document — so the chip's width
/// is the same either way. A dot that came and went would resize the chip on
/// every save and every first edit after one, shuffling the whole strip.
///
/// Centered on the label's line rather than inheriting the row's alignment:
/// the chip's own `child_align` already centers, but a dot this much shorter
/// than the text has nothing to sit on if that ever changes.
fn dirty_dot(ui: &mut Ui, theme: &Theme, tab: TabRef, dirty: bool) {
    let fill = if dirty {
        Background::rounded(theme.status.warning, Corners::all(DIRTY_DOT * 0.5))
    } else {
        // Not a transparent fill: the default paints no quad at all, which
        // is what "reserve the space, draw nothing" means here.
        Background::default()
    };
    Panel::zstack()
        .id(tab_dirty_wid(tab))
        .size((Sizing::fixed(DIRTY_DOT), Sizing::fixed(DIRTY_DOT)))
        .align(Align::v(VAlign::Center))
        .background(fill)
        .show(ui, |_| {});
}

/// Chrome lift behind a hoverable strip glyph: `header_fill` under the
/// pointer, nothing otherwise.
fn hover_bg(hovered: bool, theme: &Theme, corners: Corners) -> Background {
    if hovered {
        Background::rounded(theme.card.header_fill, corners)
    } else {
        Background::default()
    }
}

/// One strip's shared draw state, threaded through its chips (the
/// [`crate::gui::pane::graph::ctx::DrawCtx`] pattern, strip-scoped).
#[derive(Debug)]
struct StripCtx<'a> {
    theme: &'a Theme,
    /// The group a split-menu pick splits.
    group: TabGroupId,
    out: &'a mut Requests,
}

/// Draw one group's strip. Tab activate / close clicks are handled in
/// [`DockUi::scan`](super::DockUi::scan) (prepass); split-menu picks push
/// directly into `out` this frame.
pub(super) fn show(
    ui: &mut Ui,
    theme: &Theme,
    group: &TabGroup,
    labels: &[TabLabel],
    out: &mut Requests,
) {
    let mut strip = StripCtx {
        theme,
        group: group.id,
        out,
    };
    // The strip wears the chrome band; the active tab below punches
    // through to `canvas_bg` so it reads as one piece with the pane.
    Panel::hstack()
        .id(strip_wid(group.id))
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::new(6.0, 4.0, 6.0, 0.0))
        .gap(3.0)
        .child_align(Align::v(VAlign::Bottom))
        .background(Background::fill(theme.colors.chrome_fill))
        .show(ui, |ui| {
            for label in labels {
                tab_chip(ui, &mut strip, label);
            }
        });
}

fn tab_chip(ui: &mut Ui, s: &mut StripCtx<'_>, label: &TabLabel) {
    let theme = s.theme;
    let active = label.active;
    let r = theme.card.corner_radius;
    // Active-tab selection cue: a 2px accent cap along the top, built from two
    // layered backgrounds. The outer is filled with the accent and rounded to
    // the full `r`; the inner tab fill is nested `ACCENT` px lower with a
    // tighter corner, so the accent peeks out only as a thin top band that
    // follows the rounded corners. The cap wears the full accent only in the
    // focused group. Inactive tabs skip the cap and wear a faint
    // `tab_inactive` chip; the active fill stays `canvas_bg` so its bottom
    // dissolves into the pane.
    const ACCENT: f32 = 2.0;
    let outer_top = if active { ACCENT } else { 0.0 };
    let outer_bg = if active {
        let cap = if label.focused {
            theme.colors.selection_rect
        } else {
            theme.card.header_fill
        };
        Background::rounded(cap, Corners::new(r, r, 0.0, 0.0))
    } else {
        Background::default()
    };
    let inner_r = if active { (r - ACCENT).max(0.0) } else { r };
    let inner_fill = if active {
        theme.canvas.bg
    } else {
        theme.colors.tab_inactive
    };
    let inner_bg = Background::rounded(inner_fill, Corners::new(inner_r, inner_r, 0.0, 0.0));
    // A closable tab trades right inset for the top-right close button (equal
    // 4px top/right gaps); Main stays symmetric so its label is centered. The
    // active tab lifts its inner top inset by `ACCENT`, so the cap adds no
    // height and the label sits at the same place on every tab.
    let inner_top = 4.0 - outer_top;
    let padding = if closable(label.tab) {
        Spacing::new(10.0, inner_top, 4.0, 4.0)
    } else {
        Spacing::new(10.0, inner_top, 10.0, 4.0)
    };
    // Match the menu bar's smaller (13px) label scale on every tab; the
    // active tab carries full-strength ink, inactive tabs recede to muted.
    let ink = if active {
        ui.theme().text.color
    } else {
        theme.colors.text_muted
    };
    let label_style = colored_text(ui, ink, theme.text.body);
    // Outer carries the accent fill + click sense + the 2px top inset; the
    // inner carries the tab fill + content, nested `ACCENT` px lower so the
    // accent shows only as a top cap. Every chip also senses drags — the
    // docking gesture (`gui::dock::drag`); the 4 px latch threshold keeps
    // plain clicks working unchanged.
    Panel::hstack()
        .id(tab_chip_wid(label.tab))
        .size((Sizing::HUG, Sizing::HUG))
        .sense(Sense::CLICK | Sense::DRAG)
        .padding(Spacing::new(0.0, outer_top, 0.0, 0.0))
        .background(outer_bg)
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("tab_content")
                .size((Sizing::HUG, Sizing::HUG))
                .padding(padding)
                .gap(6.0)
                .child_align(Align::v(VAlign::Center))
                .background(inner_bg)
                .show(ui, |ui| {
                    // A plain label: it senses nothing, so the press falls
                    // through to the chip around it, which is what
                    // `DockUi::scan` polls for both the activation click and
                    // the drag edges.
                    Text::new(label.text).style(&label_style).show(ui);

                    if shows_document(label.tab) {
                        dirty_dot(ui, theme, label.tab, label.dirty);
                    }
                    if closable(label.tab) {
                        close_button(ui, theme, tab_close_wid(label.tab));
                    }
                });
        });

    split_menu(ui, s, label.tab);
}

/// The chip's top-right `×`. Hover comes from last frame's response; the
/// click is consumed in [`DockUi::scan`](super::DockUi::scan).
fn close_button(ui: &mut Ui, theme: &Theme, close_wid: WidgetId) {
    let bg = hover_bg(ui.response_for(close_wid).hovered, theme, Corners::all(3.0));
    Panel::zstack()
        .id(close_wid)
        .size((Sizing::fixed(16.0), Sizing::fixed(16.0)))
        .sense(Sense::CLICK)
        // Pin to the chip's top edge so it reads as a top-right corner
        // close (it's already the rightmost item in the row).
        .align(Align::v(VAlign::Top))
        .child_align(Align::CENTER)
        .background(bg)
        .show(ui, |ui| {
            // `×` at a size that fits the 16px box (the default 16px font
            // overflows and rides high). `line_height_mult: 1.0` hugs the
            // glyph (no 1.2× leading) so it centers in the 16px button
            // instead of riding high.
            let style = TextStyle {
                line_height_mult: 1.0,
                ..muted_text(ui, theme, theme.text.body)
            };
            Text::new("\u{00d7}")
                .style(&style)
                .text_align(Align::CENTER)
                .show(ui);
        });
}

/// Right-click split menu: the keyboard-free, discoverable route to the
/// same split a chip drag performs. Opens on the chip's secondary click;
/// a pick moves `tab` into a fresh pane on the chosen side.
fn split_menu(ui: &mut Ui, s: &mut StripCtx<'_>, tab: TabRef) {
    let menu_wid = tab_menu_wid(tab);
    if ui.response_for(tab_chip_wid(tab)).right.clicked()
        && let Some(p) = ui.pointer_pos()
    {
        ContextMenu::open(ui, menu_wid, p);
    }
    ContextMenu::for_id(menu_wid)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui, popup| {
            let mut side = None;
            if MenuItem::new("Split right").show(ui, popup).left.clicked() {
                side = Some(SplitSide::Right);
            }
            if MenuItem::new("Split down").show(ui, popup).left.clicked() {
                side = Some(SplitSide::Bottom);
            }
            if let Some(side) = side {
                s.out.push_view(DockOp::MoveTab {
                    tab,
                    to: DockDrop::Split {
                        group: s.group,
                        side,
                    },
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use crate::core::document::TabRef;
    use crate::core::document::harness::DocFixture;
    use crate::gui::app::session::harness::SessionHarness;
    use crate::gui::dock::strip::{DIRTY_DOT, tab_chip_wid, tab_dirty_wid};

    /// The dot is a visibility change, never a layout one: saving (or making
    /// the first edit after a save) must leave the graph chip exactly the size
    /// it was, or every chip to its right would shift.
    #[test]
    fn the_dirty_dot_reserves_the_same_box_saved_and_unsaved() {
        // Preferences beside the graph so the test also covers a chip that
        // reserves nothing: it must not grow a slot of its own.
        let mut h = SessionHarness::new(DocFixture::sample().with_tab(TabRef::Preferences));
        h.session.open.dirty = false;
        h.prime(2);
        let saved_chip =
            h.ui.rect(tab_chip_wid(TabRef::Graph))
                .expect("the graph tab records a chip");
        let saved_dot =
            h.ui.rect(tab_dirty_wid(TabRef::Graph))
                .expect("a saved document still reserves the dot's box");

        h.session.open.dirty = true;
        h.prime(2);
        let unsaved_chip = h.ui.rect(tab_chip_wid(TabRef::Graph)).expect("still there");
        let unsaved_dot =
            h.ui.rect(tab_dirty_wid(TabRef::Graph))
                .expect("still there");

        assert_eq!(
            (saved_chip.size.w, saved_chip.size.h),
            (unsaved_chip.size.w, unsaved_chip.size.h),
            "the dot resized the graph chip: {saved_chip:?} saved vs {unsaved_chip:?} unsaved",
        );
        assert_eq!(
            (saved_dot.size.w, saved_dot.size.h),
            (DIRTY_DOT, DIRTY_DOT),
            "the reserved box is the dot's own size",
        );
        assert_eq!(
            (unsaved_dot.size.w, unsaved_dot.size.h),
            (DIRTY_DOT, DIRTY_DOT),
            "an inked dot fills exactly the box the saved document reserved",
        );
        // The dot rides inside the chip that owns it, not past its edge.
        assert!(
            unsaved_dot.max().x <= unsaved_chip.max().x,
            "the dot overflowed its chip: {unsaved_dot:?} in {unsaved_chip:?}",
        );
        assert!(
            h.ui.rect(tab_dirty_wid(TabRef::Preferences)).is_none(),
            "preferences are not in the document and reserve no dot",
        );
    }
}
