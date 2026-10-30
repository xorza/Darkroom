//! The palantir-side half of the theme: darkroom's palette projected onto
//! [`palantir::Palette`], and the [`palantir::Theme`] that builds.

use palantir::{
    ButtonTheme, RadiusSteps, SpaceSteps, Stroke, StrokeSteps, TabsTheme, TextStyleOverrides,
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
    }
}

/// What the bridge needs past the palette projection: the theme's scales. A bundle rather than six
/// parameters.
#[derive(Clone, Copy, Debug)]
pub(super) struct BridgeRoles<'a> {
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
    /// every floating surface does, a tab chip rounds as a card, and every control snaps.
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
            // Darkroom's own controls snap, so palantir's do too: a button in a dialog changes as
            // a node's icon button does, and a view saved right after a menu pick pictures the
            // settled window, not a title halfway through its fade.
            transition: None,
        }
    }
}

/// Palantir sub-theme for darkroom: assemble every widget recipe from the palette and `m` via
/// [`palantir::Theme::from_parts`], then apply the darkroom-only tweaks (the type scale's body as
/// the ambient text, surround-colored dock seam, the icon set's tab close button, a wider menu).
///
/// The ambient size is [`TypeScale::body`], and no widget look pins a size
/// of its own: a palantir widget then reads the tier every darkroom label
/// at that prominence reads.
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
    theme.splitter.rule = p.surround;
    theme.splitter.rule_thickness = m.space.xs;

    tab_close(&mut theme.tabs, r);

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

/// The tab close button as the icon set's `Cross`, so it strokes as every icon does, on a
/// control-rounded lift: the top rung, the one fill that shows on a hovered chip as on the
/// canvas-filled selected one, pressed or not. The icon box is the inline tier's, as a node
/// footer's status icon is.
const fn tab_close(tabs: &mut TabsTheme, r: BridgeRoles<'_>) {
    let close = r.icon.inline;
    tabs.close_size = close.side();
    tabs.close_glyph = icon::cross_span(close.icon);
    tabs.close_stroke = icon::stroke_width(close.icon);
    tabs.close.active.chrome.border = Stroke::NONE;
}

/// A dialog's primary action: palantir's button recipe filled with the accent and its label in
/// the window's ground ink. Under the
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
