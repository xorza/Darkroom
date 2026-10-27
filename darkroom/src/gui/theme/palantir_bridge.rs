//! The palantir-side half of the theme: darkroom's palette projected onto
//! [`palantir::Palette`], and the [`palantir::Theme`] that builds.

use glam::Vec2;
use palantir::{
    ButtonTheme, Chrome, CloseShown, Corners, RgbaF32, Spacing, StatefulLook, Stroke,
    TextStyleOverrides, WidgetLook,
};

use crate::gui::theme::palette::Palette;
use crate::gui::theme::radius_scale::RadiusScale;
use crate::gui::theme::space_scale::SpaceScale;
use crate::gui::theme::type_scale::TypeScale;
use crate::gui::widgets::icon;

/// darkroom's roles as the [`palantir::Palette`] that
/// [`palantir::Theme::from_palette`] wants, so every widget palantir paints
/// reads the same palette as darkroom's own widgets. Two notes on the
/// mapping:
/// - `window_bg` is the window clear and the selected tab chip —
///   the graph canvas, so a chip dissolves into the pane below it.
/// - `element` and `node_fill` are one color by design: nodes and
///   palantir's own surfaces sit on the same tier.
pub(super) const fn palette(p: &Palette) -> palantir::Palette {
    palantir::Palette {
        text: p.text,
        text_muted: p.text_muted,
        text_disabled: p.text_disabled,
        window_bg: p.canvas_bg,
        element: p.node_fill,
        element_mid: p.elem_hover,
        element_strong: p.elem_active,
        border_focused: p.border_focused,
        accent: p.selection_rect,
    }
}

/// The darkroom colors the bridge needs past the palette projection —
/// the roles whose palantir counterpart lands on a different rung than
/// darkroom wants.
///
/// A bundle rather than five parameters: two of them are `RgbaF32`, so
/// they swap and still compile.
#[derive(Clone, Copy, Debug)]
pub(super) struct BridgeRoles<'a> {
    /// The band the menu bar, the status bar and every tab strip sit on.
    pub(super) surround_fill: RgbaF32,
    /// The selection cap on a strip that does not hold focus.
    pub(super) header_fill: RgbaF32,
    pub(super) text: &'a TypeScale,
    pub(super) space: &'a SpaceScale,
    /// The tab chip is a card; menus and the modal float; the rest are controls.
    pub(super) radius: &'a RadiusScale,
}

/// Palantir sub-theme for darkroom: assemble every widget recipe from
/// the palette via [`palantir::Theme::from_palette`], then apply the
/// darkroom-only tweaks (the type scale's body as the ambient text,
/// surround-colored dock seam, darkroom's own tab chips).
///
/// The ambient size is [`TypeScale::body`], and no widget look pins a size
/// of its own: a palantir widget then reads the tier every darkroom label
/// at that prominence reads, rather than palantir's 16 px default.
pub(super) fn theme(p: &palantir::Palette, r: BridgeRoles<'_>) -> palantir::Theme {
    let BridgeRoles {
        surround_fill,
        text,
        space,
        radius,
        ..
    } = r;
    let mut theme = palantir::Theme::from_palette(p);

    // Dock splitter: the resting seam paints the surround band that frames
    // the panes, so the gap reads as part of that surround rather than a
    // dark line (hovered/active fill still marks the grab target); a wider
    // seam does the visual separation.
    theme.splitter.rule = surround_fill;
    theme.splitter.rule_thickness = 4.0;

    tab_roles(&mut theme, p, r);

    theme.text = theme.text.with_font_size(text.body);
    theme.tooltip.text = TextStyleOverrides::NONE;
    scales(&mut theme, space, radius);
    menu(&mut theme, p, space);
    theme
}

/// A menu as the review drew it: the hint a gutter from its label, and a panel that floats on a
/// soft shadow inside a rule in the separators' ink, not on palantir's lit border. Its 32 px rows
/// are [`scales`]'s row inset, which a tab shares.
const fn menu(theme: &mut palantir::Theme, p: &palantir::Palette, space: &SpaceScale) {
    let menu = &mut theme.context_menu;
    menu.item.gap = space.xl;
    menu.min_width = MENU_MIN_WIDTH;
    menu.panel.border = Stroke::new(p.border_soft(), 1.0);
    menu.panel.shadow.offset = Vec2::new(0.0, space.sm);
    menu.panel.shadow.blur = space.lg;
}

/// A menu's width floor: room for "Save As Compressed…" and a gutter, so the File, Edit and
/// Graph menus open one width and a short menu does not read as a pill.
const MENU_MIN_WIDTH: f32 = 240.0;

/// The theme's spacing steps and radii over palantir's own literals, so a palantir control
/// lines up with a darkroom one: a field and a button beside it share one height, and a menu
/// rounds like every other floating surface.
fn scales(theme: &mut palantir::Theme, space: &SpaceScale, radius: &RadiusScale) {
    let control = Spacing::xy(space.sm, space.xs);
    theme.button.defaults.padding = Spacing::xy(space.md, space.xs);
    theme.text_edit.defaults.padding = control;
    theme.context_menu.padding = Spacing::all(space.xs);
    theme.tooltip.padding = control;
    // A tab and a menu item are one row: the same inset, so the same height at the body size.
    let row = Spacing::xy(space.md, space.sm);
    theme.context_menu.item.defaults.padding = row;
    theme.tabs.chip_padding = row;
    theme.modal.padding = Spacing::all(space.xl);

    let round = |look: &mut StatefulLook, r: f32| {
        for state in look.each_mut() {
            state.chrome.corners = Corners::all(r);
        }
    };
    round(&mut theme.button.looks, radius.control);
    round(&mut theme.text_edit.looks, radius.control);
    round(&mut theme.context_menu.item.looks, radius.control);
    theme.tooltip.panel.corners = Corners::all(radius.control);
    theme.context_menu.panel.corners = Corners::all(radius.float);
    theme.modal.panel.corners = Corners::all(radius.float);
}

/// The lift around the close button's icon box, each side.
const CLOSE_LIFT: f32 = 3.0;

/// A chip's width floor: its padding, label gap, close box and trailing inset take 42 px, so 64
/// leaves a short name room.
const TAB_MIN_WIDTH: f32 = 64.0;

/// Darkroom's own tab chips over palantir's recipe.
///
/// Palantir derives its strip from the palette alone, which lands the
/// band on `element`, the idle cap on `element_strong` and every chip on a
/// fill. Darkroom wants the strip to continue the surround band the menu bar
/// rides, ghost chips and close buttons as its other controls are, and the
/// spacing and radius scales every other surface uses.
fn tab_roles(theme: &mut palantir::Theme, p: &palantir::Palette, r: BridgeRoles<'_>) {
    let BridgeRoles {
        surround_fill,
        header_fill,
        text,
        space,
        radius,
    } = r;
    let tabs = &mut theme.tabs;
    tabs.strip = Chrome::fill(surround_fill);
    tabs.strip_padding = Spacing::new(space.sm, space.xs, space.sm, 0.0);
    tabs.gap = space.xs;
    tabs.label_gap = space.sm;
    tabs.trailing_inset = space.xs;
    tabs.min_width = TAB_MIN_WIDTH;
    tabs.radius = radius.card;
    tabs.cap_unfocused = header_fill;
    // An unselected chip is a ghost with a menu title's fills, as the two share the surround: no
    // fill at rest, `element_mid` under the pointer, `element_strong` pressed. The selected chip
    // keeps palantir's `window_bg` fill, which is darkroom's canvas: its bottom edge dissolves
    // into the pane below.
    let top = Corners::top(radius.card);
    let chip = |fill: RgbaF32| Chrome::rounded(fill, top);
    // Destructured, so a state palantir adds fails here rather than keeping its default.
    let [normal, hovered, active, disabled] = tabs.unselected.each_mut();
    normal.chrome = Chrome::NONE;
    normal.text = TextStyleOverrides::NONE.with_color(p.text_muted);
    hovered.chrome = chip(p.element_mid);
    hovered.text = TextStyleOverrides::NONE;
    active.chrome = chip(p.element_strong);
    active.text = TextStyleOverrides::NONE;
    disabled.chrome = Chrome::NONE;
    disabled.text = TextStyleOverrides::NONE.with_color(p.text_disabled);

    // The close button is the icon set's `Cross`, so it strokes as every icon does, on a
    // control-rounded lift. It shows on the selected chip and the chip under the pointer, as VS
    // Code's and Zed's strips do. Its lift is the top rung, the one fill that shows on a hovered
    // chip as on the canvas-filled selected one, pressed or not.
    // The icon box is the label tier's, as a node footer's status icon is.
    let close_icon = text.label;
    tabs.close_size = close_icon + 2.0 * CLOSE_LIFT;
    tabs.close_glyph = icon::cross_span(close_icon);
    tabs.close_stroke = icon::stroke_width(close_icon);
    tabs.close_shown = CloseShown::SelectedOrHovered;
    let lift = |fill: RgbaF32| Chrome::rounded(fill, Corners::all(radius.control));
    let [normal, hovered, active, disabled] = tabs.close.each_mut();
    normal.chrome = Chrome::NONE;
    normal.text = TextStyleOverrides::NONE.with_color(p.text_muted);
    hovered.chrome = lift(p.element_strong);
    hovered.text = TextStyleOverrides::NONE;
    active.chrome = lift(p.element_strong);
    active.text = TextStyleOverrides::NONE;
    disabled.chrome = Chrome::NONE;
    disabled.text = TextStyleOverrides::NONE.with_color(p.text_disabled);

    // The ghost chip trailing the pointer wears the surround band, so it
    // reads as a tab lifted off its strip rather than as a popup.
    theme.dock.ghost.chrome = Chrome::rounded(surround_fill, Corners::all(radius.card))
        .with_border(Stroke::new(p.accent, 1.0));
}

/// Menu-bar trigger look: palantir's [`ButtonTheme::menu_button`] recipe
/// (transparent at rest, `element_mid` / `element_strong` fills, no chip
/// overlay) with the label muted until hovered — so a trigger reads as a
/// menu, not as a button.
///
/// A darkroom slot rather than a `palantir::Theme` field because no
/// palantir widget resolves against a menu-bar style: the bar hands this
/// to [`palantir::Button::style`] at the call site.
///
/// Pins color only; every other axis inherits the ambient style an
/// unstyled label would have.
pub(super) fn menu_button(p: &palantir::Palette) -> ButtonTheme {
    let mut mb = ButtonTheme::menu_button(p);
    let restyle = |look: &mut WidgetLook, color: RgbaF32| {
        look.text = look.text.with_color(color);
    };
    let [normal, hovered, active, disabled] = mb.looks.each_mut();
    restyle(normal, p.text_muted);
    restyle(hovered, p.text);
    restyle(active, p.text);
    restyle(disabled, p.text_disabled);
    mb
}

/// `menu_button` with its resting look the hovered one, for the trigger whose menu is open.
pub(super) fn menu_button_open(menu_button: &ButtonTheme) -> ButtonTheme {
    let mut open = menu_button.clone();
    open.looks.normal = open.looks.hovered.clone();
    open
}
