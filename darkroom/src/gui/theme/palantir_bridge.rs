//! The palantir-side half of the theme: darkroom's palette projected onto
//! [`palantir::Palette`], and the [`palantir::Theme`] that builds.

use palantir::{
    ButtonTheme, Chrome, CloseShown, MenuBarTheme, RadiusSteps, RgbaF32, SpaceSteps, Stroke,
    StrokeSteps, TextStyleOverrides, WidgetLook,
};

use crate::gui::theme::color;
use crate::gui::theme::elevation_scale::ElevationScale;
use crate::gui::theme::glass;
use crate::gui::theme::icon_scale::IconScale;
use crate::gui::theme::palette::Palette;
use crate::gui::theme::radius_scale::RadiusScale;
use crate::gui::theme::space_scale::SpaceScale;
use crate::gui::theme::stroke_scale::StrokeScale;
use crate::gui::theme::type_scale::TypeScale;
use crate::gui::widgets::icon;

/// darkroom's roles as the [`palantir::Palette`] that
/// [`palantir::Theme::from_parts`] wants, so every widget palantir paints
/// reads the same palette as darkroom's own widgets. Two notes on the
/// mapping:
/// - `window_bg` is the window clear and the selected tab chip —
///   the graph canvas, so a chip dissolves into the pane below it.
/// - `element` and `node_bg` are one color by design: nodes and
///   palantir's own surfaces sit on the same tier.
pub(super) const fn palette(p: &Palette) -> palantir::Palette {
    palantir::Palette {
        text: p.text,
        text_muted: p.text_muted,
        text_disabled: p.text_disabled,
        window_bg: p.canvas_bg,
        element: p.node_bg,
        element_mid: p.element_mid,
        element_strong: p.element_strong,
        // The accent, 5.19:1 on a node, where a darker blue read 1.99:1 on a field's fill,
        // under the 3:1 WCAG 1.4.11 asks of a focus indicator.
        border_focused: p.accent,
        accent: p.accent,
    }
}

/// What the bridge needs past the palette projection: the surround, whose palantir counterpart
/// lands on another rung than darkroom wants, and the theme's scales. A bundle rather than seven
/// parameters.
#[derive(Clone, Copy, Debug)]
pub(super) struct BridgeRoles<'a> {
    /// The band the menu bar, the status bar and every tab strip sit on.
    pub(super) surround_bg: RgbaF32,
    pub(super) font: &'a TypeScale,
    pub(super) space: &'a SpaceScale,
    pub(super) radius: &'a RadiusScale,
    pub(super) stroke: &'a StrokeScale,
    pub(super) icon: &'a IconScale,
    pub(super) elevation: &'a ElevationScale,
}

impl BridgeRoles<'_> {
    /// The theme's steps as palantir's recipes read them, so a palantir control lines up with a
    /// darkroom one: a field and a button beside it share one height, a menu rounds and casts as
    /// every floating surface does, and a tab chip rounds as a card.
    pub(super) const fn metrics(&self) -> palantir::Metrics {
        let Self {
            space,
            radius,
            stroke,
            elevation,
            ..
        } = *self;
        palantir::Metrics {
            space: SpaceSteps {
                xs: space.xs,
                sm: space.sm,
                md: space.md,
                lg: space.lg,
                xl: space.xl,
            },
            radius: RadiusSteps {
                mark: radius.mark,
                control: radius.control,
                card: radius.card,
                float: radius.float,
            },
            stroke: StrokeSteps {
                hairline: stroke.hairline,
                fine: stroke.fine,
                line: stroke.line,
            },
            float_shadow: elevation.floating,
        }
    }
}

/// Palantir sub-theme for darkroom: assemble every widget recipe from the palette and `m` via
/// [`palantir::Theme::from_parts`], then apply the darkroom-only tweaks (the type scale's body as
/// the ambient text, surround-colored dock seam, darkroom's own tab chips, a wider menu).
///
/// The ambient size is [`TypeScale::body`], and no widget look pins a size
/// of its own: a palantir widget then reads the tier every darkroom label
/// at that prominence reads, rather than palantir's 16 px default.
pub(super) fn theme(
    p: &palantir::Palette,
    m: &palantir::Metrics,
    r: BridgeRoles<'_>,
) -> palantir::Theme {
    let mut theme = palantir::Theme::from_parts(p, m);

    // Dock splitter: the resting seam paints the surround band that frames
    // the panes, so the gap reads as part of that surround rather than a
    // dark line (hovered/active fill still marks the grab target); a wider
    // seam does the visual separation.
    theme.splitter.rule = r.surround_bg;
    theme.splitter.rule_thickness = m.space.xs;

    tab_roles(&mut theme, p, r);
    menu_bar_roles(&mut theme.menu_bar, p, m);

    theme.text = theme.text.with_font_size(r.font.body);
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
        glass::frost(panel, p.text_muted);
    }
    theme
}

/// A menu's width floor: room for "Save As…" and its Ctrl+Shift+S hint a gutter apart, so the
/// File, Edit and Graph menus open one width and a short menu does not read as a pill.
const MENU_MIN_WIDTH: f32 = 240.0;

/// A chip's width floor: its padding, label gap, close box and trailing inset take 42 px, so 64
/// leaves a short name room.
const TAB_MIN_WIDTH: f32 = 64.0;

/// Darkroom's own tab chips over palantir's recipe.
///
/// Palantir puts the strip on no band, the idle cap on `element_strong` and
/// every chip on a fill. Darkroom wants the strip to continue the surround band
/// the menu bar rides, and ghost chips and close buttons as its other controls
/// are.
fn tab_roles(theme: &mut palantir::Theme, p: &palantir::Palette, r: BridgeRoles<'_>) {
    let tabs = &mut theme.tabs;
    tabs.strip = Chrome::fill(r.surround_bg);
    tabs.min_width = TAB_MIN_WIDTH;
    // An unselected chip is a ghost with a menu title's fills, as the two share the surround: no
    // fill at rest, `element_mid` under the pointer, `element_strong` pressed. The selected chip
    // keeps palantir's `window_bg` fill, which is darkroom's canvas: its bottom edge dissolves
    // into the pane below.
    // Destructured, so a state palantir adds fails here rather than keeping its default.
    let [normal, hovered, active, disabled] = tabs.unselected.each_mut();
    normal.chrome = Chrome::NONE;
    hovered.chrome.fill = p.element_mid.into();
    hovered.text = TextStyleOverrides::NONE;
    active.chrome.fill = p.element_strong.into();
    disabled.chrome = Chrome::NONE;

    // The close button is the icon set's `Cross`, so it strokes as every icon does, on a
    // control-rounded lift. It shows on the selected chip and the chip under the pointer, as VS
    // Code's and Zed's strips do. Its lift is the top rung, the one fill that shows on a hovered
    // chip as on the canvas-filled selected one, pressed or not.
    // The icon box is the inline tier's, as a node footer's status icon is.
    let close = r.icon.inline;
    tabs.close_size = close.side();
    tabs.close_glyph = icon::cross_span(close.icon);
    tabs.close_stroke = icon::stroke_width(close.icon);
    tabs.close_shown = CloseShown::SelectedOrHovered;
    tabs.close.active.chrome.border = Stroke::NONE;

    // The ghost chip trailing the pointer wears the surround band, so it
    // reads as a tab lifted off its strip rather than as a popup.
    theme.dock.ghost.chrome.fill = r.surround_bg.into();
}

/// The menu bar's titles: palantir's flat recipe with the label muted until hovered, so a title
/// reads as a menu, not as a button, the open title in the hovered label as in its fill, and an
/// `xs` gutter between titles. Pins color; every other axis inherits the ambient style an
/// unstyled label would have.
fn menu_bar_roles(bar: &mut MenuBarTheme, p: &palantir::Palette, m: &palantir::Metrics) {
    bar.gap = m.space.xs;
    let restyle = |look: &mut WidgetLook, color: RgbaF32| {
        look.text = look.text.with_color(color);
    };
    let [normal, hovered, active, disabled] = bar.title.each_mut();
    restyle(normal, p.text_muted);
    restyle(hovered, p.text);
    restyle(active, p.text);
    restyle(disabled, p.text_disabled);
    restyle(&mut bar.open, p.text);
}

/// A dialog's primary action: palantir's button recipe filled with the accent and its label in
/// the window's ground ink, as `menu_bar_roles` restyles the menu bar's from palantir's. Under the
/// pointer it lifts as a port does; pressed, it drops back to the accent.
pub(super) fn primary_button(p: &palantir::Palette, m: &palantir::Metrics) -> ButtonTheme {
    let mut button = ButtonTheme::from_parts(p, m);
    let on_accent = TextStyleOverrides::NONE.with_color(p.window_bg);
    // Destructured, so a state palantir adds fails here rather than keeping its default.
    let [normal, hovered, active, _disabled] = button.looks.each_mut();
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
