//! Shared chrome for floating view toolbars: the frosted group pill the
//! [`Chip`](crate::gui::widgets::chip::Chip) buttons ride on. Used by the
//! graph canvas toolbar and the image viewer's control panel; each caller
//! keeps its own glyphs and toggle color policy.

use palantir::prelude::*;

use crate::gui::theme::Theme;

/// Opacity of a group pill's frosted chrome backdrop. Keeps the toolbar
/// readable over an empty canvas *and* over content it happens to sit on —
/// the backdrop color sits between the canvas and node fills, so a bit of
/// translucency still contrasts against both while the content stays
/// faintly visible through it.
const PILL_BG_ALPHA: f32 = 0.7;
/// The frosted chrome backdrop shared by toolbar group pills.
pub(crate) fn chrome(theme: &Theme) -> Chrome {
    Chrome::rounded(
        theme.colors.surround_fill.with_alpha(PILL_BG_ALPHA),
        Corners::all(theme.radius.float),
    )
}

/// One frosted group pill: `panel` (a caller-configured `Panel::hstack`
/// / `vstack` carrying its id and any alignment) dressed in the shared
/// pill layout — hugging, chip gap, pill padding, frosted backdrop — for the
/// caller to show. The pill senses pointer gestures itself, so a drag or
/// scroll starting between chips stays on the pill instead of falling
/// through to the canvas or image beneath.
pub(crate) fn dress(panel: Panel, theme: &Theme) -> Panel {
    panel
        .size((Sizing::HUG, Sizing::HUG))
        .gap(theme.space.xs)
        .padding(Spacing::all(theme.space.xs))
        .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL)
        .chrome(chrome(theme))
}

/// Thin horizontal rule between concept groups sharing one column
/// (vstack) pill, inset from the pill's chrome on both ends. Grow an axis
/// parameter when a row pill first needs one.
pub(crate) fn rule(ui: &mut Ui, theme: &Theme) {
    Separator::horizontal()
        .color(theme.colors.border_soft())
        .margin(Spacing::new(theme.space.xs, 0.0, theme.space.xs, 0.0))
        .show(ui);
}
