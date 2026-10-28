//! The palantir-side half of the theme: darkroom's palette projected onto
//! [`palantir::Palette`], and the [`palantir::Theme`] that builds.

use palantir::{
    BackdropFilter, ButtonTheme, Chrome, CloseShown, RadiusSteps, RgbaF32, SpaceSteps, Stroke,
    StrokeSteps, TextStyleOverrides, WidgetLook,
};

use crate::gui::theme::color;
use crate::gui::theme::elevation_scale::ElevationScale;
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
        frost(panel, p.text_muted);
    }
    theme
}

/// A menu's width floor: room for "Save As…" and its Ctrl+Shift+S hint a gutter apart, so the
/// File, Edit and Graph menus open one width and a short menu does not read as a pill.
const MENU_MIN_WIDTH: f32 = 240.0;

/// The tint's alpha over a popup's frosted backdrop: the canvas reads through as a blurred wash
/// under the popup's own color.
const POPUP_TINT_ALPHA: f32 = 0.72;

/// The contrast WCAG 1.4.3 asks of text against its ground
/// (<https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum>).
const TEXT_CONTRAST: f32 = 4.5;

/// A popup's blur σ in logical px: wide enough that a node's title under a popup reads as color,
/// not as text competing with the rows.
const POPUP_BLUR: f32 = 20.0;

/// `panel` over frosted glass: its own color as the tint, made translucent, as Fluent puts
/// acrylic under its transient, light-dismiss surfaces
/// (<https://learn.microsoft.com/windows/apps/develop/ui/in-app-acrylic>) and macOS its menus
/// on a blurred material.
///
/// The glass shows at most the luminance that keeps `ink`, the dimmest ink the popup paints, at
/// [`TEXT_CONTRAST`] over any backdrop. The brightest ground `ink` holds it on is
/// `g = (Y(ink) + 0.05) / 4.5 − 0.05`, and the ground over a backdrop at the ceiling `c` is
/// `(1 − α)·c + α·Y(tint)`, both in the linear light palantir composites in; so
/// `c = (g − α·Y(tint)) / (1 − α)`.
fn frost(panel: &mut Chrome, ink: RgbaF32) {
    let tint = panel
        .fill
        .as_solid()
        .expect("a popup panel is a solid fill");
    let ground = (color::luminance(ink) + 0.05) / TEXT_CONTRAST - 0.05;
    let tint_luminance = color::luminance(tint);
    debug_assert!(
        tint_luminance <= ground,
        "the ink holds its contrast on the opaque panel"
    );
    let ceiling = (ground - POPUP_TINT_ALPHA * tint_luminance) / (1.0 - POPUP_TINT_ALPHA);
    panel.fill = tint.with_alpha(POPUP_TINT_ALPHA).into();
    panel.backdrop_filter = BackdropFilter::frosted(POPUP_BLUR).with_max_luminance(ceiling);
}

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

/// A menu-bar trigger's two looks, built beside each other so they cannot drift.
#[derive(Clone, Debug)]
pub(crate) struct MenuButtonLooks {
    /// The trigger whose menu is shut.
    pub(crate) shut: ButtonTheme,
    /// The trigger whose menu is open: lit at rest as when hovered, as macOS, Windows and GNOME
    /// light the title of the open menu.
    pub(crate) open: ButtonTheme,
}

impl MenuButtonLooks {
    pub(super) fn new(p: &palantir::Palette, m: &palantir::Metrics) -> Self {
        let shut = menu_button(p, m);
        let mut open = shut.clone();
        open.looks.normal = open.looks.hovered.clone();
        Self { shut, open }
    }
}

/// Menu-bar trigger look: palantir's [`ButtonTheme::menu_button`] recipe
/// (transparent at rest, `element_mid` / `element_strong` fills, no chip
/// overlay) with the label muted until hovered — so a trigger reads as a
/// menu, not as a button.
///
/// Pins color; every other axis inherits the ambient style an unstyled label
/// would have.
fn menu_button(p: &palantir::Palette, m: &palantir::Metrics) -> ButtonTheme {
    let mut mb = ButtonTheme::menu_button(p, m);
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

/// A dialog's primary action: palantir's button recipe filled with the accent and its label in
/// the window's ground ink, as `menu_button` derives the menu bar's from palantir's. Under the
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
