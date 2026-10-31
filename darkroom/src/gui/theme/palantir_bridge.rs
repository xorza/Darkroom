//! The palantir-side half of the theme: darkroom's palette projected onto
//! [`palantir::Palette`], and the [`palantir::Theme`] that builds.

use palantir::{ButtonTheme, Stroke, TabsTheme, TextStyleOverrides};

use crate::gui::theme;
use crate::gui::theme::color;
use crate::gui::theme::icon_scale::IconScale;
use crate::gui::theme::palette::Palette;
use crate::gui::widgets::icon;

/// darkroom's roles as the [`palantir::Palette`] that
/// [`palantir::Theme::from_parts`] wants, so every widget palantir paints
/// reads the same palette as darkroom's own widgets. Two notes on the
/// mapping:
/// - `window_bg` is the window clear and the selected tab chip —
///   the graph canvas, so a chip dissolves into the pane below it; `surround`
///   is the band the menu bar, the status bar and every tab strip sit on.
/// - `element` and `node_bg` are one color by design: nodes and
///   palantir's own surfaces sit on the same tier.
pub(super) const fn palette(p: &Palette) -> palantir::Palette {
    palantir::Palette {
        text: p.text,
        text_muted: p.text_muted,
        text_disabled: p.text_disabled,
        window_bg: p.canvas_bg,
        surround: p.surround_bg,
        element: p.node_bg,
        element_mid: p.element_mid,
        element_strong: p.element_strong,
        // The accent, 5.19:1 on a node, where a darker blue read 1.99:1 on a field's fill,
        // under the 3:1 WCAG 1.4.11 asks of a focus indicator.
        border_focused: p.accent,
        accent: p.accent,
        // The canvas's ink on the accent, as palantir's dark palette sets it: 8.16:1, where the
        // body text read 1.89:1 as an on switch's knob.
        on_accent: p.canvas_bg,
        shadow: p.shadow,
    }
}

/// Palantir sub-theme for darkroom: assemble every widget recipe from the palette and `m` via
/// [`palantir::Theme::from_parts`], then apply the darkroom-only tweaks (a tooltip at the body
/// tier, surround-colored dock seam, the icon set's tab close button, a wider menu).
///
/// The ambient size is `m`'s body tier, which `from_parts` sets, and no widget look pins a size
/// of its own once the tooltip's caption tier is dropped: a palantir widget then reads the tier
/// every darkroom label at that prominence reads.
pub(super) fn theme(
    p: &palantir::Palette,
    m: &palantir::Metrics,
    icon: &IconScale,
) -> palantir::Theme {
    let mut theme = palantir::Theme::from_parts(p, m);

    // Dock splitter: the resting seam paints the surround band that frames
    // the panes, so the gap reads as part of that surround rather than a
    // dark line (hovered/active fill still marks the grab target); a wider
    // seam does the visual separation.
    theme.splitter.rule = p.surround;
    theme.splitter.rule_thickness = m.space.xs;

    tab_close(&mut theme.tabs, icon);

    theme.tooltip.text = TextStyleOverrides::NONE;
    let menu = &mut theme.context_menu;
    menu.min_width = MENU_MIN_WIDTH;
    // The panel sits inside a rule in the separators' ink, not on palantir's lit border.
    menu.panel.border.color = p.border_soft();
    // Every light-dismiss popup is frosted: the menu panel, which the menus, the node palette and
    // the combo lists wear, the tooltip and the color picker. A modal keeps its opaque panel: it
    // dims what is behind it under a scrim, so there is nothing to see through.
    for panel in [
        &mut theme.context_menu.panel,
        &mut theme.tooltip.panel,
        &mut theme.color_picker.popup,
    ] {
        *panel = theme::frosted(panel, p.text_muted);
    }
    theme
}

/// A menu's width floor: room for "Save As…" and its Ctrl+Shift+S hint a gutter apart, so the
/// File, Edit and Graph menus open one width and a short menu does not read as a pill.
const MENU_MIN_WIDTH: f32 = 240.0;

/// The tab close button as the icon set's `Cross`, so it strokes as every icon does, on a
/// control-rounded lift: the top rung, the one fill that shows on a hovered chip as on the
/// canvas-filled selected one, pressed or not. The icon box is the inline tier's, as a node
/// footer's status icon is.
const fn tab_close(tabs: &mut TabsTheme, icon: &IconScale) {
    let close = icon.inline;
    tabs.close_size = close.side();
    tabs.close_glyph_span = icon::cross_span(close.icon);
    tabs.close_glyph_width = icon::stroke_width(close.icon);
    tabs.close.active.chrome.border = Stroke::NONE;
}

/// A dialog's primary action: palantir's button recipe filled with the accent and its label in
/// the window's ground ink. Under the
/// pointer it lifts as a port does; pressed, it drops back to the accent.
pub(super) fn primary_button(p: &palantir::Palette, m: &palantir::Metrics) -> ButtonTheme {
    let mut button = ButtonTheme::from_parts(p, m);
    let on_accent = TextStyleOverrides::NONE.with_color(p.window_bg);
    // Destructured, so a state palantir adds fails here rather than keeping its default.
    let [normal, hovered, active, _disabled] = button.unselected.each_mut();
    for (look, fill) in [
        (normal, p.accent),
        (hovered, color::hover_lift(p.accent)),
        (active, p.accent),
    ] {
        look.chrome.fill = fill.into();
        look.chrome.border.color = fill;
        look.text = on_accent;
    }
    button
}
