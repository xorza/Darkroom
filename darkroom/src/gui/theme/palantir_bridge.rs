//! The palantir-side half of the theme: darkroom's palette projected onto
//! [`palantir::Palette`], and the [`palantir::Theme`] that builds.

use palantir::{ButtonTheme, Color, TextStyle, WidgetLook};

use crate::gui::theme::palette::Palette;
use crate::gui::theme::type_scale::TypeScale;

/// darkroom's roles as the [`palantir::Palette`] that
/// [`palantir::Theme::from_palette`] wants, so every widget palantir paints
/// reads the same palette as darkroom-owned chrome. Two notes on the
/// mapping:
/// - `terminal_bg` wants the editor / terminal surface — the graph canvas.
/// - `elem` and `node_fill` are one colour by design: nodes and palantir's
///   own surfaces sit on the same tier.
pub(super) fn palantir_palette_for(p: &Palette) -> palantir::Palette {
    palantir::Palette {
        text: p.text,
        text_muted: p.text_muted,
        text_disabled: p.text_disabled,
        terminal_bg: p.canvas_bg,
        elem: p.node_fill,
        elem_mid: p.elem_mid,
        elem_strong: p.elem_strong,
        border_focused: p.border_focused,
        accent: p.selection_rect,
    }
}

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
    // dark line (hovered/active fill still marks the grab target); a wider
    // seam does the visual separation.
    theme.splitter.rule = chrome_fill;
    theme.splitter.rule_thickness = 4.0;

    // Context-menu rows at the smaller menu scale, each keeping the colour
    // its own state resolved to.
    let base = theme.text;
    let shrink = |look: &mut WidgetLook| {
        let style = look.text.take().unwrap_or(base);
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
/// (transparent at rest, `elem_mid` / `elem_strong` fills, no chip
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
    fallback_text: TextStyle,
    text: &TypeScale,
) -> ButtonTheme {
    let mut mb = ButtonTheme::menu_button(p);
    let restyle = |look: &mut WidgetLook, color: Color| {
        let style = look.text.take().unwrap_or(fallback_text);
        look.text = Some(style.with_color(color).with_font_size(text.body));
    };
    restyle(&mut mb.looks.normal, p.text_muted);
    restyle(&mut mb.looks.hovered, p.text);
    restyle(&mut mb.looks.active, p.text);
    restyle(&mut mb.looks.disabled, p.text_disabled);
    mb
}
