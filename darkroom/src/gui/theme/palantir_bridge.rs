//! The palantir-side half of a preset: the two [`palantir::Palette`]
//! rosters and the [`palantir::Theme`] each one builds.

use palantir::{ButtonTheme, Color, TextStyle, WidgetLook};

// Layout dimensions are palette-independent — dark and light pull the same
// numbers. Each one's value lives on `Theme::build` (its field carries the doc
// comment); only the few read by more than one builder earn a name here. Font
// sizes are palette-independent too, and live on `TypeScale::DEFAULT`.
use crate::gui::theme::swatches::{dark, light};
use crate::gui::theme::type_scale::TypeScale;

/// The [`palantir::Palette`] each preset hands to
/// [`palantir::Theme::from_palette`], filled from the preset's swatches
/// so swapping dark ⇄ light recolours every widget palantir paints, not
/// just darkroom-owned chrome. Notes on the mapping:
/// - `terminal_bg` wants the editor / terminal surface — the same
///   swatch as the graph canvas in both themes.
/// - `elem` and our `NODE_FILL` are the same swatch by design: nodes
///   and palantir surfaces sit on the same surface tier.
pub(super) const PALANTIR_DARK: palantir::Palette = palantir::Palette {
    text: dark::PAL_TEXT,
    text_muted: dark::TEXT_MUTED,
    text_disabled: dark::PAL_TEXT_DISABLED,
    terminal_bg: dark::CANVAS_BG,
    elem: dark::NODE_FILL,
    elem_hover: dark::PAL_ELEM_HOVER,
    elem_active: dark::PAL_ELEM_ACTIVE,
    border_focused: dark::PAL_BORDER_FOCUSED,
    accent: dark::SELECTION_RECT,
};

/// Light peer of [`PALANTIR_DARK`] — same mapping over `light::*`.
pub(super) const PALANTIR_LIGHT: palantir::Palette = palantir::Palette {
    text: light::PAL_TEXT,
    text_muted: light::TEXT_MUTED,
    text_disabled: light::PAL_TEXT_DISABLED,
    terminal_bg: light::CANVAS_BG,
    elem: light::NODE_FILL,
    elem_hover: light::PAL_ELEM_HOVER,
    elem_active: light::PAL_ELEM_ACTIVE,
    border_focused: light::PAL_BORDER_FOCUSED,
    accent: light::SELECTION_RECT,
};

/// Palantir sub-theme for darkroom: assemble every widget recipe from
/// the palette via [`palantir::Theme::from_palette`], then apply the
/// darkroom-only tweaks (smaller context-menu font, chrome-coloured dock
/// seam).
///
/// Takes `text` rather than reaching for a private menu-font const: menu rows
/// are ordinary UI text, so they read [`TypeScale::body`] like every other
/// surface at that tier.
pub(super) fn palantir_theme_for(
    p: &palantir::Palette,
    chrome_fill: Color,
    text: &TypeScale,
) -> palantir::Theme {
    let mut theme = palantir::Theme::from_palette(p);

    // Dock splitter: the resting seam paints the chrome band that frames
    // the panes, so the gap reads as part of that surround rather than a
    // dark line (hover/drag fill still marks the grab target); a wider
    // seam does the visual separation.
    theme.splitter.rule = chrome_fill;
    theme.splitter.rule_thickness = 4.0;

    // Context-menu rows at the smaller menu scale, each keeping the colour
    // its own state resolved to.
    let base = &theme.text;
    let shrink = |look: &mut WidgetLook| {
        let style = look.text.take().unwrap_or_else(|| base.clone());
        look.text = Some(style.with_font_size(text.body));
    };
    let item = &mut theme.context_menu.item;
    shrink(&mut item.looks.normal);
    shrink(&mut item.looks.hovered);
    shrink(&mut item.looks.active);
    shrink(&mut item.looks.disabled);
    theme
}

/// Menu-bar trigger look: palantir's [`ButtonTheme::menu_button`] recipe
/// (transparent at rest, `elem_hover` / `elem_active` fills, no chip
/// overlay) with the label muted until hovered and every state at the
/// menu scale — so a trigger reads as a menu, not as a button.
///
/// A darkroom slot rather than a `palantir::Theme` field because no
/// palantir widget resolves against a menu-bar style: the bar hands this
/// to [`palantir::Button::style`] at the call site.
///
/// `fallback_text` is the assembled palantir theme's ambient style — the
/// one an unstyled label would have inherited — so the only axes this
/// pins are the two it sets.
pub(super) fn menu_button_for(
    p: &palantir::Palette,
    fallback_text: &TextStyle,
    text: &TypeScale,
) -> ButtonTheme {
    let mut mb = ButtonTheme::menu_button(p);
    let restyle = |look: &mut WidgetLook, color: Color| {
        let style = look.text.take().unwrap_or_else(|| fallback_text.clone());
        look.text = Some(style.with_color(color).with_font_size(text.body));
    };
    restyle(&mut mb.looks.normal, p.text_muted);
    restyle(&mut mb.looks.hovered, p.text);
    restyle(&mut mb.looks.active, p.text);
    restyle(&mut mb.looks.disabled, p.text_disabled);
    mb
}
