//! Shared chrome for floating view toolbars: the frosted group pill and
//! the square glyph buttons riding on it. Used by the graph canvas
//! toolbar and the image viewer's control panel; each caller keeps its
//! own glyphs and toggle color policy.

use aperture::{
    Background, Color, Configure, Corners, Panel, Rect, Sense, Separator, Shape, Sizing, Spacing,
    Stroke, Tooltip, Ui, WidgetId,
};

use crate::gui::theme::Theme;

/// Side of each square button, in px.
const BUTTON_SIZE: f32 = 30.0;
/// Inset of a toolbar from its view's corner.
pub(crate) const TOOLBAR_MARGIN: f32 = 8.0;
/// Gap between buttons.
pub(crate) const BUTTON_GAP: f32 = 6.0;
/// Corner radius of a button's rounded-rect background.
const BUTTON_RADIUS: f32 = 6.0;
/// Opacity of a group pill's frosted chrome backdrop. Keeps the toolbar
/// readable over an empty canvas *and* over content it happens to sit on —
/// the backdrop color sits between the canvas and node fills, so a bit of
/// translucency still contrasts against both while the content stays
/// faintly visible through it.
const PILL_BG_ALPHA: f32 = 0.7;
/// Padding between a group pill's chrome edge and the buttons inside it.
const PILL_PADDING: f32 = 4.0;
/// Corner radius of a group pill's chrome backdrop — the button radius
/// grown by the padding so the pill's rounding stays concentric with the
/// buttons'.
const PILL_RADIUS: f32 = BUTTON_RADIUS + PILL_PADDING;

/// The frosted chrome backdrop shared by toolbar group pills.
pub(crate) fn pill_background(theme: &Theme) -> Background {
    Background {
        fill: theme.colors.chrome_fill.with_alpha(PILL_BG_ALPHA).into(),
        corners: Corners::all(PILL_RADIUS),
        ..Default::default()
    }
}

/// One frosted group pill: `panel` (a caller-configured `Panel::hstack`
/// / `vstack` carrying its id and any alignment) dressed in the shared
/// pill chrome — hugging, chip gap, pill padding, frosted backdrop. The
/// pill senses pointer gestures itself, so a drag or scroll starting
/// between chips stays on the pill instead of falling through to the
/// canvas or image beneath.
pub(crate) fn pill(ui: &mut Ui, theme: &Theme, panel: Panel, body: impl FnOnce(&mut Ui)) {
    panel
        .size((Sizing::Hug, Sizing::Hug))
        .gap(BUTTON_GAP)
        .padding(Spacing::all(PILL_PADDING))
        .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL)
        .background(pill_background(theme))
        .show(ui, body);
}

/// Thin horizontal rule between concept groups sharing one column
/// (vstack) pill, inset from the pill chrome on both ends. Grow an axis
/// parameter when a row pill first needs one.
pub(crate) fn pill_rule(ui: &mut Ui, theme: &Theme) {
    const INSET: f32 = 5.0;
    Separator::horizontal()
        .color(theme.colors.text_muted.with_alpha(0.18))
        .margin(Spacing::new(INSET, 0.0, INSET, 0.0))
        .show(ui);
}

/// One square glyph toggle, an opaque chip raised off the group pill.
/// `toggled` inverts it (`toggled_fill` with a dark glyph); idle is a
/// neutral fill (lighter on hover) with the caller's `idle_glyph` ink.
/// Returns whether it was clicked.
#[expect(clippy::too_many_arguments)]
pub(crate) fn toggle_button(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    toggled: bool,
    idle_glyph: Color,
    toggled_fill: Color,
    tip: &'static str,
    draw_glyph: impl FnOnce(&mut Ui, f32, Color),
) -> bool {
    let hovered = ui.response_for(wid).hovered;
    // Glyph and fill vary on different axes: the glyph only inverts for
    // the toggled state, the fill also lifts on hover.
    let glyph = if toggled {
        theme.colors.chrome_fill
    } else {
        idle_glyph
    };
    let fill = if toggled {
        toggled_fill
    } else if hovered {
        theme.colors.header_fill
    } else {
        theme.colors.node_fill
    };
    glyph_button(ui, wid, fill, glyph, tip, draw_glyph)
}

/// One square momentary button (no toggled state), an opaque chip raised
/// off the group pill: neutral fill that lifts on hover, with a muted
/// glyph. Returns whether it was clicked.
pub(crate) fn action_button(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    tip: &'static str,
    draw_glyph: impl FnOnce(&mut Ui, f32, Color),
) -> bool {
    let hovered = ui.response_for(wid).hovered;
    let fill = if hovered {
        theme.colors.header_fill
    } else {
        theme.colors.node_fill
    };
    glyph_button(ui, wid, fill, theme.colors.text_muted, tip, draw_glyph)
}

/// Shared square-button body: a `fill` rounded-rect background, the icon
/// painted centered in the `BUTTON_SIZE` box by `draw_glyph` in `glyph`,
/// and a hover `tip`. Returns whether it was clicked this frame.
fn glyph_button(
    ui: &mut Ui,
    wid: WidgetId,
    fill: Color,
    glyph: Color,
    tip: &'static str,
    draw_glyph: impl FnOnce(&mut Ui, f32, Color),
) -> bool {
    let s = BUTTON_SIZE;
    let button = Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(s), Sizing::Fixed(s)))
        .sense(Sense::CLICK)
        .background(Background {
            fill: fill.into(),
            corners: Corners::all(BUTTON_RADIUS),
            ..Default::default()
        })
        .show(ui, |ui| draw_glyph(ui, s, glyph));
    // Take the owned snapshot + click result so the button's `ui` borrow
    // ends before the tooltip records into `ui`.
    let snapshot = button.response.snapshot();
    let clicked = button.response.clicked();
    Tooltip::for_(&snapshot).text(tip).show(ui);
    clicked
}

/// The shared rounded-rect outline glyphs frame their contents in.
pub(crate) fn frame(ui: &mut Ui, s: f32, color: Color) {
    let w = s * 0.62;
    let o = (s - w) * 0.5;
    stroked_rect(ui, Rect::new(o, o, w, w), s * 0.08, color, s * 0.06);
}

/// A rounded-rect outline (transparent fill, `color` stroke of `width`).
pub(crate) fn stroked_rect(ui: &mut Ui, rect: Rect, radius: f32, color: Color, width: f32) {
    ui.add_shape(Shape::RoundedRect {
        local_rect: Some(rect),
        corners: Corners::all(radius),
        fill: Color::TRANSPARENT.into(),
        stroke: Stroke::solid(color, width),
    });
}

/// A small filled circle of radius `r` centered at `(cx, cy)`.
pub(crate) fn dot(ui: &mut Ui, cx: f32, cy: f32, r: f32, color: Color) {
    ui.add_shape(Shape::RoundedRect {
        local_rect: Some(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r)),
        corners: Corners::all(r),
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
}
