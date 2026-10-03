//! Shared chrome for floating view toolbars: the frosted group pill the
//! [`Chip`](crate::gui::widgets::chip::Chip) buttons ride on. Used by the
//! graph canvas toolbar and the image viewer's control panel; each caller
//! keeps its own glyphs and toggle color policy.

use palantir::prelude::*;

use crate::gui::theme::Theme;
use crate::gui::widgets::chip::BUTTON_RADIUS;

/// Inset of a toolbar from its view's corner.
pub(crate) const TOOLBAR_MARGIN: f32 = 8.0;
/// Gap between buttons.
pub(crate) const BUTTON_GAP: f32 = 6.0;
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
    Background::rounded(
        theme.colors.chrome_fill.with_alpha(PILL_BG_ALPHA),
        Corners::all(PILL_RADIUS),
    )
}

/// One frosted group pill: `panel` (a caller-configured `Panel::hstack`
/// / `vstack` carrying its id and any alignment) dressed in the shared
/// pill chrome — hugging, chip gap, pill padding, frosted backdrop. The
/// pill senses pointer gestures itself, so a drag or scroll starting
/// between chips stays on the pill instead of falling through to the
/// canvas or image beneath.
pub(crate) fn pill(ui: &mut Ui, theme: &Theme, panel: Panel, body: impl FnOnce(&mut Ui)) {
    panel
        .size((Sizing::HUG, Sizing::HUG))
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
        .color(theme.colors.border_soft())
        .margin(Spacing::new(INSET, 0.0, INSET, 0.0))
        .show(ui);
}
