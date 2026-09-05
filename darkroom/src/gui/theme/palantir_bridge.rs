//! The palantir-side half of the theme: darkroom's palette projected onto
//! [`palantir::Palette`], and the [`palantir::Theme`] that builds.

use palantir::{Background, ButtonTheme, Corners, RgbaF32, Stroke, TextStyle, WidgetLook};

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

/// The darkroom colours the bridge needs past the palette projection —
/// the roles whose palantir counterpart lands on a different rung than
/// darkroom wants.
///
/// A bundle rather than five parameters: four of them are `RgbaF32`, so
/// any two of them swap and still compile.
#[derive(Clone, Copy, Debug)]
pub(super) struct BridgeRoles<'a> {
    /// The band the menu bar, the status bar and every tab strip sit on.
    pub(super) chrome_fill: RgbaF32,
    /// Resting fill of an unselected tab chip.
    pub(super) tab_inactive: RgbaF32,
    /// The selection cap on a strip that does not hold focus, and the
    /// chrome lift behind a hovered strip glyph.
    pub(super) header_fill: RgbaF32,
    /// The unsaved-changes dot.
    pub(super) warning: RgbaF32,
    /// How round a card is — a tab chip is one.
    pub(super) corner_radius: f32,
    pub(super) text: &'a TypeScale,
}

/// Palantir sub-theme for darkroom: assemble every widget recipe from
/// the palette via [`palantir::Theme::from_palette`], then apply the
/// darkroom-only tweaks (smaller context-menu font, chrome-coloured dock
/// seam, darkroom's own tab chips).
///
/// Takes the type scale rather than reaching for a private menu-font
/// const: menu rows are ordinary UI text, so they read
/// [`TypeScale::body`] like every other surface at that tier.
pub(super) fn palantir_theme_for(p: &palantir::Palette, r: BridgeRoles<'_>) -> palantir::Theme {
    let BridgeRoles {
        chrome_fill, text, ..
    } = r;
    let mut theme = palantir::Theme::from_palette(p);

    // Dock splitter: the resting seam paints the chrome band that frames
    // the panes, so the gap reads as part of that surround rather than a
    // dark line (hovered/active fill still marks the grab target); a wider
    // seam does the visual separation.
    theme.splitter.rule = chrome_fill;
    theme.splitter.rule_thickness = 4.0;

    tab_roles(&mut theme, p, r);

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

/// Darkroom's own tab chips over palantir's recipe.
///
/// Palantir derives its strip from the palette alone, which lands the
/// band on `elem` and the idle cap on `elem_strong`. Darkroom wants the
/// strip to continue the chrome band the menu bar rides, an unselected
/// chip on its own `tab_inactive` rung, and the card radius every other
/// elevated surface uses — so those five roles are set here and the rest
/// of the recipe is left alone.
fn tab_roles(theme: &mut palantir::Theme, p: &palantir::Palette, r: BridgeRoles<'_>) {
    let tabs = &mut theme.tabs;
    tabs.strip = Background::fill(r.chrome_fill);
    tabs.corner = r.corner_radius;
    tabs.accent_idle = r.header_fill;
    tabs.badge = r.warning;
    // The selected chip keeps palantir's `terminal_bg` fill, which is
    // darkroom's canvas: its bottom edge dissolves into the pane below.
    let top = Corners::top(r.corner_radius);
    let chip = |fill: RgbaF32| Background::rounded(fill, top);
    tabs.inactive.normal.background = chip(r.tab_inactive);
    tabs.inactive.hovered.background = chip(p.elem_mid);
    tabs.inactive.active.background = chip(p.elem_strong);
    tabs.inactive.disabled.background = chip(p.elem);
    // The chrome lift behind a hovered close button is the same header
    // band a node's title wears.
    let lift = Background::rounded(r.header_fill, Corners::all(3.0));
    tabs.close.hovered.background = lift.clone();
    tabs.close.active.background = lift;
    // Chips at the menu scale, like every other chrome label.
    let base = theme.text;
    let scale = |look: &mut WidgetLook| {
        let style = look.text.unwrap_or(base);
        look.text = Some(style.with_font_size(r.text.body));
    };
    scale(&mut theme.tabs.active.normal);
    scale(&mut theme.tabs.active.hovered);
    scale(&mut theme.tabs.active.active);
    scale(&mut theme.tabs.active.disabled);
    scale(&mut theme.tabs.inactive.normal);
    scale(&mut theme.tabs.inactive.hovered);
    scale(&mut theme.tabs.inactive.active);
    scale(&mut theme.tabs.inactive.disabled);

    // The ghost chip trailing the pointer wears the chrome band, so it
    // reads as a tab lifted off its strip rather than as a popup.
    theme.dock.ghost.background = Background::rounded(r.chrome_fill, Corners::all(4.0))
        .with_stroke(Stroke::solid(p.accent, 1.0));
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
    let restyle = |look: &mut WidgetLook, color: RgbaF32| {
        let style = look.text.take().unwrap_or(fallback_text);
        look.text = Some(style.with_color(color).with_font_size(text.body));
    };
    restyle(&mut mb.looks.normal, p.text_muted);
    restyle(&mut mb.looks.hovered, p.text);
    restyle(&mut mb.looks.active, p.text);
    restyle(&mut mb.looks.disabled, p.text_disabled);
    mb
}
